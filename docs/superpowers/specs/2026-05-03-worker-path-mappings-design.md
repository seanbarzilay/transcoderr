# Per-Worker Path Mappings — Design Spec

**Status:** approved (brainstorm 2026-05-03)
**Scope:** single-PR feature, additive on top of the existing remote dispatch path.
**Predecessors:** distributed-transcoding roadmap (Pieces 1–6) merged through v0.37.0; worker auto-discovery shipped as v0.38.0 (PR #95).

## Problem

Today the deploy docs explicitly require both sides to mount the media volume at the same absolute path:

> Mount the media volume at the same path the coordinator uses — the worker reads/writes the file directly.

This forces homogeneous filesystem layouts across the cluster. Realistic deployments break that assumption all the time:

- Coordinator on a NAS host: `/mnt/movies/X.mkv`.
- Remote GPU worker on a Linux desktop: `/data/media/movies/X.mkv` (same NFS share, mounted at a different root).
- Another remote worker in a Docker container: `/srv/transcode/X.mkv`.

There is no good reason to forbid this. Both sides already speak the same protocol; the only thing that diverges is the path string. The coordinator should be able to translate paths on the wire on a per-worker basis, transparently to the worker.

## Goal

The operator configures, per remote worker, a list of prefix mappings: "every path that starts with `<from>` on my side should be `<to>` on this worker's side." The coordinator's dispatcher rewrites paths in both directions transparently. Workers don't change. Plugins don't change. Today's homogeneous deployments keep working unchanged because the default state (no mappings) is identity translation.

## Decisions (locked in brainstorm)

- **Q1-B — Translation scope: walk-the-tree prefix replace.** The dispatcher walks the entire `Context` JSON tree and rewrites every string leaf whose value starts with a configured `from:` prefix. Operator-specific prefixes (e.g. `/mnt/movies`) make false positives vanishingly unlikely. Schema-aware translation (requiring every plugin to declare `format: "path"` fields) is rejected as out of scope: zero plugins declare it today and forcing every author to opt in is heavy for a feature that only needs to handle filesystem paths in a fixed `Context` shape.
- **Q2-A — Storage: JSON column on the existing `workers` table.** New nullable column `path_mappings_json TEXT NULL`. Stores `[{"from": "...", "to": "..."}, ...]`. Matches the existing `hw_caps_json` / `plugin_manifest_json` precedent on the same table. Longest-prefix-match wins; the array order doesn't matter to the resolver.

The two decisions above cover the whole design space; everything below is implementation detail.

## Architecture

```
┌────────────────────────────┐                ┌──────────────────────────────┐
│ Coordinator                │                │ Remote worker                │
│                            │                │                              │
│  Engine builds Context     │                │  receives StepDispatch       │
│  ctx.file.path = "/mnt/    │                │  ctx.file.path = "/data/     │
│      movies/X.mkv"         │                │      media/movies/X.mkv"     │
│  ↓                         │                │  ↓                           │
│  RemoteRunner::run         │                │  step.execute(ctx)           │
│  ↓                         │                │  ↓ (worker writes its        │
│  load workers.path_        │                │     output to               │
│   mappings_json for this   │                │     /data/media/movies/      │
│   worker_id                │                │     X.transcoded.mkv)        │
│  ↓                         │                │                              │
│  apply forward(ctx, rules) │                │  returns ctx with            │
│   walks JSON tree,         │                │  ctx.steps.tx.output_path =  │
│   rewrites string leaves   │                │   "/data/media/movies/       │
│   /mnt/movies → /data/     │                │      X.transcoded.mkv"       │
│   media/movies             │                │                              │
│  ↓                         │ ───StepDispatch──►                            │
│                            │                │                              │
│                            │ ◄──StepComplete──                             │
│  apply reverse(            │                │                              │
│    returned ctx, rules)    │                │                              │
│   walks JSON tree,         │                │                              │
│   rewrites /data/media/    │                │                              │
│   movies → /mnt/movies     │                │                              │
│  ↓                         │                │                              │
│  next step on coordinator  │                │                              │
│  reads                     │                │                              │
│  ctx.steps.tx.output_path  │                │                              │
│  = "/mnt/movies/           │                │                              │
│     X.transcoded.mkv"      │                │                              │
└────────────────────────────┘                └──────────────────────────────┘
```

Mappings are captured as a snapshot at dispatch time and reused for the reverse pass when `StepComplete` arrives. Mid-flight edits to the worker's mappings cannot desync a step's round-trip.

## Components

### Backend

- **Migration `migrations/<date>_worker_path_mappings.sql`:**
  ```sql
  ALTER TABLE workers ADD COLUMN path_mappings_json TEXT;
  ```
  Nullable. Default = NULL (identity, current behavior). No backfill needed.

- **`crates/transcoderr/src/path_mapping.rs`** — new pure-data module:
  ```rust
  pub struct PathMapping { pub from: String, pub to: String }

  pub struct PathMappings { rules: Vec<PathMapping> } // sorted by from.len() desc

  pub enum Direction { CoordToWorker, WorkerToCoord }

  impl PathMappings {
      pub fn from_json(s: &str) -> anyhow::Result<Self>;
      pub fn is_empty(&self) -> bool;
      pub fn apply(&self, value: &mut serde_json::Value, dir: Direction);
  }
  ```
  - `apply` recurses through `Object` / `Array`; on every `String` leaf, finds the longest matching rule and rewrites in place.
  - **Boundary rule:** a rule with `from = "/mnt/movies"` matches `"/mnt/movies"` exactly OR `"/mnt/movies/anything"`, but **NOT** `"/mnt/movies-archive/Y.mkv"`. Implementation: after stripping `from` from the leading edge, the next char must be `/` or end-of-string.
  - Reverse direction swaps `from` ↔ `to` at apply time, no separate sorted vector.
  - Object keys are not rewritten (path values live in keys' values, not the keys themselves).

- **`crates/transcoderr/src/db/workers.rs`:**
  - `WorkerRow` gains `pub path_mappings_json: Option<String>` (`#[sqlx(default)]` so existing tests stay green).
  - New `pub async fn update_path_mappings(pool, id, json: Option<&str>) -> anyhow::Result<u64>` — refuses `kind='local'` rows (returns `Ok(0)` and the API turns that into a 400). Empty array → store NULL.
  - All existing `SELECT` queries gain `path_mappings_json` in the column list.

- **`crates/transcoderr/src/dispatch/remote.rs`** — `RemoteRunner::run` extended with two hooks:
  ```rust
  // 1. Load mappings for this worker (cached on Connections registry,
  //    refreshed only when path_mappings_json changes via the API).
  let mappings = state.connections.path_mappings_for(worker_id).await
      .unwrap_or_default(); // None → identity

  // 2. At dispatch time, walk the snapshot before sending.
  let mut snap_value: serde_json::Value =
      serde_json::from_str(&ctx.to_snapshot())?;
  if !mappings.is_empty() {
      mappings.apply(&mut snap_value, Direction::CoordToWorker);
  }
  let ctx_snapshot = serde_json::to_string(&snap_value)?;

  // ... StepDispatch with ctx_snapshot ...

  // 3. On StepComplete, reverse-apply before installing.
  if let Some(snap) = c.ctx_snapshot {
      let mut snap_value: serde_json::Value = serde_json::from_str(&snap)?;
      if !mappings.is_empty() {
          mappings.apply(&mut snap_value, Direction::WorkerToCoord);
      }
      let cancel = ctx.cancel.clone();
      *ctx = Context::from_snapshot(&snap_value.to_string())?;
      ctx.cancel = cancel;
  }
  ```

- **`crates/transcoderr/src/worker/connections.rs`** — `Connections` registry gains a small per-worker mappings cache. Loaded on first dispatch to that worker; invalidated by the API endpoint when mappings change.

- **`crates/transcoderr/src/api/workers.rs`** — new endpoint `PUT /api/workers/:id/path-mappings`:
  - Body: `{ "rules": [ { "from": "...", "to": "..." }, ... ] }`.
  - Authenticated (existing `protected` Router branch — same auth as the rest of `/api/workers`).
  - Validates: each rule has non-empty `from` and `to`; otherwise 400.
  - Refuses `kind='local'` rows (400).
  - Empty `rules` array → stores NULL (clears mappings).
  - On success, invalidates the `Connections` mappings cache for that `worker_id` and returns `{"id", "rules": [...]}`.
  - Existing `GET /api/workers` returns `path_mappings: [{from, to}, ...]` (or `null`) on each worker — un-redacted (no secrets).

### Web UI

- **`web/src/pages/workers.tsx`** — for `kind='remote'` workers, gain a new "Edit mappings" button next to the existing per-worker controls.
- **`web/src/components/path-mappings-modal.tsx`** — new modal:
  - Header: "Path mappings — `<worker name>`".
  - Body: a table of rows, each with two text inputs (`From`, `To`) and a delete (✕) button.
  - "Add mapping" appends a fresh row.
  - "Save" PUTs to `/api/workers/:id/path-mappings`. Empty list = clear all.
  - Reuses the existing `.modal-*` CSS classes (added in install-log-modal).
- **`web/src/api/client.ts`** — new `api.workers.updatePathMappings(id, rules)` method.
- The setup-wizard flow (PR #80) is unaffected — mappings are configured after enrollment.

## Wire flow (concrete example)

Assume `worker_id = 7` has rules:
```json
[
  { "from": "/mnt/movies", "to": "/data/media/movies" },
  { "from": "/mnt/tv",     "to": "/data/media/tv" }
]
```

Engine starts a `transcode` step with:
```
ctx = {
  file: { path: "/mnt/movies/X.mkv", size_bytes: 12345678 },
  steps: {},
  ...
}
```

**Forward (dispatch):**
- The walker visits `ctx.file.path` (string `"/mnt/movies/X.mkv"`).
- Longest matching rule by `from.len()` desc: `/mnt/movies` matches at offset 0; next char is `/` → boundary OK.
- Rewrite to `/data/media/movies/X.mkv`. Walker does not descend into `String`.
- `ctx.file.size_bytes` is a number → no rewrite.
- `ctx.steps` is an empty object → walker descends, finds nothing.

`StepDispatch.ctx_snapshot` carries `/data/media/movies/X.mkv`.

**Worker:**
- Receives the dispatch, runs ffmpeg over `/data/media/movies/X.mkv`, writes the transcoded output to `/data/media/movies/X.transcoded.mkv`, populates:
  ```
  ctx.steps.tx.output_path = "/data/media/movies/X.transcoded.mkv"
  ```
- Sends `StepComplete` with the new ctx_snapshot.

**Reverse (completion):**
- Walker visits `ctx.file.path` (`/data/media/movies/X.mkv`) — applies reverse mapping (`/data/media/movies` → `/mnt/movies`) → `/mnt/movies/X.mkv`.
- Walker visits `ctx.steps.tx.output_path` (`/data/media/movies/X.transcoded.mkv`) — same reverse mapping → `/mnt/movies/X.transcoded.mkv`.
- The next step on the coordinator (`output { mode: replace }`) reads the now-coordinator-space path and operates on the local file. ✓

## Edge cases

| Case | Behavior |
|---|---|
| Worker `path_mappings_json = NULL` | `PathMappings::from_json` returns `Self::default()` (empty rules). Walker is short-circuited via `is_empty()`. Identity. |
| Worker created via API with `rules: []` | API stores `NULL`. Same as above. |
| `kind='local'` worker | Local dispatch never goes through `RemoteRunner`. UI hides the button; API returns 400. |
| Trailing slash on `from` (`/mnt/movies/`) | Boundary rule treats `/mnt/movies` and `/mnt/movies/` as equivalent for matching purposes — both match `/mnt/movies/X.mkv`. Operator may use either form interchangeably. (We lightly normalize on save: strip trailing `/` from both `from` and `to` so display is consistent. Empty input is still 400.) |
| Operator edits mappings while a step is in flight | The `RemoteRunner` snapshots the mapping list at dispatch time. Round-trip uses the dispatch-time mappings. The next step picks up the edit. |
| String happens to look like a path but isn't | If the operator's `from:` prefix matches it, it'll be rewritten. In practice, prefixes are operator-specific media roots (`/mnt/movies`), so collisions in non-path strings are vanishingly unlikely. We accept this trade-off vs the complexity of schema-aware rewriting. |
| `from` overlaps with another rule's `from` | Longest-`from` wins. E.g. with both `/mnt/movies` → `/data/media/movies` AND `/mnt/movies/4k` → `/data/4k`, a path of `/mnt/movies/4k/X.mkv` rewrites via the second rule. |
| `to` is shorter than `from` (downsizing) | Works fine. The walker rewrites in place; reverse is symmetric. |
| Auto-discovery worker connects | `path_mappings_json` defaults to NULL on enrollment (the `/api/worker/enroll` endpoint doesn't set it). Operator adds mappings via the UI afterward. |

## Testing

### Unit (`path_mapping::tests`)

- `empty_mappings_is_identity` — apply with no rules leaves the value untouched.
- `single_rule_rewrites_string_leaf` — `ctx.file.path` is rewritten end-to-end.
- `longest_prefix_wins` — two overlapping rules; the longer one is applied to a path covered by both.
- `path_component_boundary_respected` — `from = "/mnt/movies"` does NOT rewrite `"/mnt/movies-archive/Y.mkv"`.
- `reverse_round_trip` — `apply(forward); apply(reverse)` returns the original JSON byte-for-byte.
- `walks_nested_objects_and_arrays` — paths inside `ctx.steps.tx.output_path` and inside arrays are rewritten.
- `non_string_leaves_untouched` — numbers / bools / nulls / object keys ignored.
- `trailing_slash_normalisation` — `from = "/mnt/movies/"` and `from = "/mnt/movies"` produce identical match behavior.

### Integration (`tests/path_mapping.rs`)

- Boot a fake remote worker; configure `path_mappings_json = [{"from": "/coord", "to": "/worker"}]` for it via `update_path_mappings`. Submit a job whose `ctx.file.path = "/coord/movies/X.mkv"`.
  - Assert the `StepDispatch` envelope received by the fake worker has `ctx.file.path = "/worker/movies/X.mkv"`.
  - Have the fake worker reply with a `ctx_snapshot` containing `ctx.steps.tx.output_path = "/worker/movies/X.transcoded.mkv"`.
  - Assert the coordinator's post-receive `ctx` has `output_path = "/coord/movies/X.transcoded.mkv"` (reverse rewrite).

### API (`tests/worker_path_mappings_api.rs`)

- `PUT /api/workers/:id/path-mappings` with valid rules → 200 + GET round-trips.
- `PUT` with `kind='local'` worker → 400.
- `PUT` with empty `from` or empty `to` → 400.
- `PUT` with `rules: []` → 200 + GET shows `path_mappings = null`.
- Mappings cache is invalidated: `PUT` then immediate dispatch sees the new mappings (covered by the integration test above plus a focused unit test on `Connections::set_path_mappings`).

## Migration / backward compat

- Existing workers (homogeneous-mount deployments) get NULL mappings = identity. Behavior unchanged.
- No protocol change. Workers are completely unaware of mappings.
- Existing `GET /api/workers` response gains a new `path_mappings` field; clients that ignore unknown fields are unaffected. Web UI is updated in lock-step.
- Setup wizard (PR #80) and auto-discovery (PR #95) flows remain unchanged.

## Dependencies

No new crates. Pure additions to existing modules.

## Out of scope

- Coordinator ↔ arr-source path translation (different problem; the *arr instances push their own filesystem paths to the coordinator's webhook endpoint and that's a separate translation surface).
- Glob / regex patterns. Prefix is sufficient for realistic deployments.
- Per-flow / per-step mappings. Per-worker is the right granularity (a worker has one filesystem; a step's path requirement is implicitly determined by where the step runs).
- Schema-driven path detection on `with` payloads — would require every plugin to declare `format: "path"` (none do today).
- "Smart" path matching (canonical-path resolution, symlink following, etc.). The walker operates on the literal string in the JSON snapshot.
