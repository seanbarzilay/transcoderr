# Distributed Transcoding — Piece 2: Local-worker Abstraction

## Goal

Make the in-process worker pool register through the same mechanism remote
workers use, so the local worker becomes "just another worker" with
`kind=local`. Add a per-worker enable/disable toggle, surfaced as a row
control on the `/workers` UI page. No job-routing changes — Piece 3 wires
that up.

## Roadmap context

- Roadmap parent: `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md` (merged in PR #81).
- Piece 1 (wire protocol skeleton + worker daemon): merged in PR #83 as
  `v0.32.0`. The `workers` table, the seeded `local` row (`id=1, kind='local',
  enabled=1`), the `db::workers` CRUD, the WS upgrade endpoint, the REST
  endpoints, the auth-middleware extension, the Workers UI page, and the
  4-scenario integration test all landed.
- This piece (Piece 2) is the bridge between Piece 1's data model and Piece
  3's per-step routing: it makes the local worker's row look like a real
  registered worker so the dispatcher in Piece 3 has uniform inputs.

## Locked-in decisions (from brainstorming)

1. **Registration is in-process** — at boot, the coordinator writes
   directly to the `local` row via the existing `db::workers::record_register`.
   No loopback WS. Token-less (the local row's `secret_token` stays NULL).
2. **Disable behavior is graceful drain** — the currently-running job
   finishes; the next `claim_next` short-circuits. Disabling is not "kill
   the job", it's "stop accepting more". (Hard-pause + reassign-on-disable
   is Piece 6 territory.)
3. **Heartbeat fires every 30s regardless of enabled state.** A disabled
   local worker still stamps `last_seen_at` so the UI distinguishes
   "operator turned it off" (`enabled=false`) from "the daemon is dead"
   (`last_seen_at` stale).
4. **Enable/disable lives per-row on the `/workers` page.** The
   `workers.enabled` column added in Piece 1 was put there for this; the
   per-row toggle keeps the abstraction uniform — when Piece 6 lets you
   disable individual remote workers, it's the same control.
5. **No new DB migration.** Piece 1 already added every column this piece
   needs (`enabled`, `hw_caps_json`, `plugin_manifest_json`,
   `last_seen_at`).

## Architecture

### Boot wiring

`main.rs::serve` (or wherever the boot path runs after `db::open` and the
ffmpeg-caps probe) calls a single new function:

```rust
crate::worker::local::register_local_worker(
    &pool,
    &ffmpeg_caps,
    &discovered_plugins,
).await;
```

Internally it:

1. Builds an `hw_caps_json` JSON string from `FfmpegCaps` (same shape the
   remote worker daemon ships in Piece 1's `Register` payload — for now
   `{"has_libplacebo": <bool>}`, refined when Piece 3 fills `hw_caps`
   out).
2. Builds a `plugin_manifest_json` JSON string (`Vec<PluginManifestEntry>`,
   with `name`, `version`, `sha256: None`).
3. Calls `db::workers::record_register(LOCAL_WORKER_ID, hw_caps_json,
   manifest_json)`.

Failure logs `tracing::warn!` and continues — boot must not block on this.

After registration, `main.rs` spawns the heartbeat task:

```rust
crate::worker::local::spawn_local_heartbeat(pool.clone());
```

### Heartbeat task

A `tokio::spawn` background task running:

```text
loop {
    sleep(30s);
    if let Err(e) = db::workers::record_heartbeat(LOCAL_WORKER_ID) {
        tracing::warn!(error=?e, "local heartbeat failed");
    }
}
```

Mirrors the remote workers' 30s `HEARTBEAT_INTERVAL`. Fires regardless of
`enabled` — the daemon being alive is independent of "should this worker
claim jobs."

### Pool gating

`worker::pool::Worker::run_loop` is the existing claim-and-run loop. The
change is one new check before each `claim_next`:

```rust
if !crate::worker::local::is_enabled(&self.pool).await {
    tokio::select! {
        _ = shutdown.changed() => return,
        _ = tokio::time::sleep(Duration::from_millis(500)) => continue,
    }
}
```

`is_enabled` returns `true` on DB error so transient sqlite hiccups don't
stall work. The 500ms re-check matches the existing idle backoff — when
the operator re-enables, the next loop iteration picks it up within a
half-second.

### Toggle endpoint

New `PATCH /api/workers/:id` in the **protected** router chain. Body:

```json
{"enabled": true|false}
```

Handler at `crates/transcoderr/src/api/workers.rs`:

```rust
pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<PatchReq>,
) -> Result<Json<WorkerSummary>, StatusCode>;
```

- 200 + the updated `WorkerSummary` on success.
- 404 if id missing.
- 400 on malformed body.
- 500 on db error.

Works uniformly for local + remote rows. Calls
`db::workers::set_enabled(pool, id, enabled)` (new function).

### UI

`web/src/pages/workers.tsx` table grows an "Enabled" column with a
toggle. The toggle calls `api.workers.patch(id, {enabled})` and
invalidates the `["workers"]` query.

Status badge logic updates:

```text
if !enabled:                         "disabled"   (.badge-disabled)
else if last_seen_at == null:        "offline"
else if age < STALE_AFTER_SECS (90): "connected"
else:                                "stale"
```

`.badge-disabled` is a new CSS rule alongside the existing
`.badge-connected` / `.badge-stale` / `.badge-offline`.

The local row's Delete column stays empty (delete is remote-only — the
existing `db::workers::delete_remote` filter still protects this).

## File structure

**New backend files:**

- `crates/transcoderr/src/worker/local.rs` — local-worker registration,
  heartbeat task, enable check
- `crates/transcoderr/tests/local_worker.rs` — integration tests

**Modified backend files:**

- `crates/transcoderr/src/worker/mod.rs` — `pub mod local;`
- `crates/transcoderr/src/worker/pool.rs` — `run_loop` consults `is_enabled`
- `crates/transcoderr/src/db/workers.rs` — add `set_enabled`, plus the
  unit test for it
- `crates/transcoderr/src/api/workers.rs` — add `PatchReq` + `patch` handler
- `crates/transcoderr/src/api/mod.rs` — register the PATCH route in the
  protected chain
- `crates/transcoderr/src/main.rs` — call `register_local_worker` after
  the existing ffmpeg/plugin discovery, spawn `spawn_local_heartbeat`

**Modified web files:**

- `web/src/pages/workers.tsx` — toggle column, badge update
- `web/src/api/client.ts` — `api.workers.patch(id, body)` wrapper
- `web/src/types.ts` — likely no change (`Worker` already has `enabled:
  boolean`); add `WorkerPatchReq` if helpful
- `web/src/index.css` — `.badge-disabled` rule

## Wire / API additions

### `PATCH /api/workers/:id`

Protected (auth required). Body:

```json
{"enabled": true}
```

Response 200, `application/json`:

```json
{
  "id": 1,
  "name": "local",
  "kind": "local",
  "secret_token": null,
  "hw_caps": {"has_libplacebo": false},
  "plugin_manifest": [],
  "enabled": true,
  "last_seen_at": 1747850000,
  "created_at": 1747800000
}
```

Errors: 400 (malformed body), 404 (id not found), 500 (db error). Auth
401/403 by the existing middleware.

The GET response shape doesn't change — `enabled: bool` is already there
since Piece 1.

## Database

No schema migration. The existing fields on the `workers` table are
sufficient:

- `hw_caps_json` (TEXT, nullable) — populated at boot
- `plugin_manifest_json` (TEXT, nullable) — populated at boot
- `last_seen_at` (INTEGER, nullable, unix seconds) — heartbeat stamps
  every 30s
- `enabled` (INTEGER, NOT NULL DEFAULT 1) — toggled by PATCH

A constant `pub const LOCAL_WORKER_ID: i64 = 1;` lives in
`worker/local.rs` — pinned to the migration's `INSERT ... VALUES ('local',
'local', 1, ...)` which gets `rowid=1`. If anyone ever changes the
seeded-row insert order, the constant moves with it.

## Existing-install migration path

For the Piece 1 → Piece 2 upgrade, no schema change is needed; the
`local` row exists with `name='local'`, `kind='local'`, `enabled=1`,
`secret_token=NULL`, `hw_caps_json=NULL`, `plugin_manifest_json=NULL`,
`last_seen_at=NULL`. The first boot of the Piece 2 binary populates
`hw_caps_json` + `plugin_manifest_json` via `record_register` and starts
heartbeating. From the operator's perspective, the existing local
worker silently grows a "connected" badge on the Workers page.

## Error handling

- **Boot registration** — failure logs `tracing::warn!`, doesn't abort
  startup. The local pool still works; the UI just shows stale data
  until the next register attempt (which doesn't auto-retry within
  Piece 2 — operator restart is the recovery path; not worth
  engineering today).
- **Heartbeat task** — failure logs `tracing::warn!`, the loop continues
  on the next 30s tick. A persistently-failing DB will spam warns but
  not crash the daemon.
- **`is_enabled` DB error** — defaults to `true`. Stalling work on a
  transient sqlite issue is a worse failure than letting the worker
  keep claiming.
- **PATCH endpoint errors** — 400/404/500 mapped per body / row /
  query failure.

## Testing

### Unit tests

In `crates/transcoderr/src/db/workers.rs`:

1. `set_enabled_round_trips` — toggle 1 → 0 → 1 and assert the column.

In `crates/transcoderr/src/worker/local.rs`:

2. `is_enabled_returns_column_value` — seed enabled=1, assert true; seed
   enabled=0, assert false.
3. `is_enabled_defaults_true_on_db_error` — pass a closed pool, assert
   true. (Optional; only if it's easy to fabricate.)

### Integration tests (`crates/transcoderr/tests/local_worker.rs`)

Reuses the existing `common::boot()` helper.

1. **`local_row_populated_after_boot`**
   - `boot()` calls register + spawns heartbeat (Piece 2's main.rs change).
   - Assert `hw_caps_json IS NOT NULL`, `plugin_manifest_json IS NOT
     NULL`, `last_seen_at IS NOT NULL` for the local row.
2. **`heartbeat_advances_last_seen_when_idle`**
   - Capture initial `last_seen_at`.
   - Sleep 1.1s (so unix-second granularity advances).
   - Force a heartbeat tick (test helper that calls
     `db::workers::record_heartbeat(LOCAL_WORKER_ID)` — we don't wait
     30s in tests).
   - Assert `last_seen_at > initial`.
3. **`disabled_local_worker_drains_and_stops_claiming`**
   - PATCH /api/workers/1 → `{"enabled": false}`.
   - Submit a flow + a job (use existing test helpers).
   - Wait 2s.
   - Assert the job's status is still `pending`.
4. **`re_enabling_resumes_dispatch`**
   - From state (3), PATCH back to `{"enabled": true}`.
   - Wait up to 2s.
   - Assert the job's status is `running` or `completed`.

### Existing tests must stay green

The Piece 1 integration tests (`worker_connect.rs`, 4 scenarios), the
auth tests (7), the concurrent-claim and crash-recovery tests, and the
full lib suite (~150) all run unchanged. The pool's `run_loop` change is
behavioral — gated by `is_enabled` which defaults to true on the seeded
row, so default behavior is identical to today.

## Out of scope

- **Custom name for local worker** — stays "local" until a future piece
  adds rename UI.
- **Multiple local workers per host** — not a thing; the seeded row is
  unique.
- **Per-step routing / `run_on:`** — Piece 3.
- **Plugin push from coordinator to remote workers** — Piece 4.
- **Job reassignment when local toggles off mid-flight** — Piece 6. For
  now the running job runs to completion before the toggle takes
  effect.
- **WS notifications when `enabled` flips** — the polling re-check at
  500ms is fine for Piece 2's UX.
- **Local worker delete** — DB layer (`delete_remote` already refuses)
  and UI (Delete column already conditional on `kind === "remote"`)
  both prevent it; no Piece 2 change needed.

## Risks

- **Race during the boot register.** If the existing pool's `run_loop`
  starts before `register_local_worker` writes, a `claim_next` could
  fire against `local` row data that hasn't been stamped yet. Mitigation
  is straightforward: call `register_local_worker` synchronously before
  spawning `Worker::run_loop` in `main.rs`. The plan should make this
  ordering explicit.
- **`is_enabled` polling adds an extra DB read per tick.** Today the
  pool issues `claim_next` (one read + maybe one write); the new check
  adds one more read every 500ms. Sqlite handles this trivially — well
  under a millisecond — but it's worth noting. If profiling ever flags
  it, we cache the value in an `AtomicBool` updated by the PATCH
  handler. Not Piece 2 work.
- **Heartbeat task survives stale connections to a destroyed pool.** If
  the pool is dropped (which doesn't happen in practice — it lives the
  process lifetime), the heartbeat task would fail every 30s. Same shape
  as the existing `spawn_idle_sweep` task in `api/workers.rs`; not a
  Piece 2 concern.

## Success criteria

1. After boot of a new install: the `/workers` UI shows the `local` row
   with status "connected", a real "X seconds ago" last-seen, and the
   hardware capability summary populated from the live ffmpeg probe.
2. Clicking the local row's Enabled toggle to off → the table immediately
   shows status "disabled" → no new jobs claim from the local pool. A
   currently-running job finishes naturally.
3. Clicking it back to on → status "connected" → next job claims within
   ~500ms.
4. `PATCH /api/workers/:id` works for the local row and (separately) for
   remote rows, returning the updated `WorkerSummary`. Deletion of the
   local row is still refused (via the `delete_remote` filter), enabling
   it on a remote that hasn't connected yet still works.
5. All Piece 1 integration tests + the full existing suite stay green.

## Branch / PR

Branch: `feat/distributed-piece-2` from main. Spec branch is
`spec/distributed-piece-2` (this file). Single PR per piece, matching
Piece 1's pattern.
