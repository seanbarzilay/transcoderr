# Distributed Transcoding — Design Spec (Roadmap)

**Date:** 2026-05-02
**Status:** Roadmap. Implementation lands across 6 separate spec → plan → PR cycles. This document is the canonical reference for all of them.
**Issue:** #79 — *Distributed transcoding using worker nodes*
**Author:** Brainstorming session, 2026-05-02

## Goal

Let an operator add networked worker nodes that run ffmpeg / heavy
plugin steps for the existing transcoderr instance. The single-host
default must keep working unchanged for users who don't want this.

## Locked-in decisions

From brainstorming, 2026-05-02:

- **Asymmetric: coordinator + workers.** The existing transcoderr
  instance stays the brain (DB, web UI, sources, notifiers, flows,
  flow engine). Workers are dumb add-ons running ffmpeg / plugin
  steps. Not peer-to-peer; not split into separate binaries.
- **The coordinator is its own worker by default.** Disable-able once
  a remote worker is connected. From the dispatcher's POV, all
  workers (local + remote) look the same.
- **Shared filesystem assumption** for media. Same constraint as
  Radarr → transcoderr today. Not building file shipping.
- **WebSocket wire protocol**, worker as the dialing client (so
  workers can sit behind NAT). Bearer auth via per-worker tokens.
- **Per-step routing**, configurable. Built-in steps and plugin
  manifests declare a default executor (`coordinator` or `any-worker`).
  Operator overrides per-step in flow YAML via `run_on:`.
- **Coordinator pushes plugins to workers.** Not a shared `plugins/`
  mount (race risk on deps); not independent worker catalogs (sync
  burden). Coordinator is the source of truth; workers fetch tarballs
  from the coordinator and install via the existing pipeline.
- **Manual discovery.** Worker config carries `coordinator_url` +
  `coordinator_token`. mDNS / auto-discover deferred.

## Architecture

### Roles

- **Coordinator.** Existing transcoderr instance. New responsibilities:
  workers registry; dispatcher routes remote-eligible steps; pushes
  plugin install state to workers.
- **Worker.** New `transcoderr worker` subcommand on the same binary.
  WebSocket client, hw probe, per-device semaphores, ffmpeg/ffprobe
  invocation, subprocess plugin runtime. No DB, no web UI, no
  sources/notifiers/flows.
- **Local worker.** The coordinator's existing in-process worker
  pool, refactored to register through the same mechanism a remote
  worker uses (`kind=local`, fixed row id, no auth token). Toggleable
  enabled/disabled in Settings. Default = enabled.

### Connection

- Worker dials `wss://<coordinator>/api/worker/connect` with
  `Authorization: Bearer <worker-token>`.
- One token per worker; minted in the coordinator's UI under
  Settings → Workers (one-time-display, like API tokens).
- WebSocket carries: registration, heartbeats, job dispatch
  (coord → worker), progress + completion (worker → coord), plugin
  sync (coord → worker).
- Reconnect on drop with exponential backoff (1s → 2s → 4s → … →
  30s capped).

### Step routing

- Each step kind has a default executor. Built-ins:
  - **`coordinator`**: `probe`, `plan.*` mutators, `notify`, `output`,
    `verify.playable`, `size.report.before/after`. Cheap; need DB
    access (notify) or are pure data manipulation against probe
    results.
  - **`any-worker`**: `plan.execute`, `transcode`, `remux`,
    `extract.subs`, `iso.extract`, `audio.ensure`, `strip.tracks`.
    All ffmpeg-heavy.
- Plugin manifests gain optional `executor = "coordinator" |
  "any-worker"` (default `coordinator`).
- Operator overrides per-step in flow YAML:
  ```yaml
  - use: plan.execute
    run_on: coordinator   # or any-worker
  ```
  Per-specific-worker pinning (`run_on: gpu-box-1`) deferred —
  YAGNI for v1.
- `any-worker` with no available remote AND local disabled →
  step blocks pending. Unblocks when capacity arrives.

### File transport

Shared filesystem. Same path on coordinator and workers, mounted via
NFS/SMB/Tailscale/etc. Documented constraint, not enforced — if a
worker can't read the input file path, the step fails with
`webhook: file not found at /path` and the run goes to `on_failure`.

## Wire protocol

### Envelope

```json
{ "type": "<kind>", "id": "<uuid>", "payload": {...} }
```

JSON text frames. `id` is a worker-side correlation id for
request/response pairs (e.g. `step_dispatch` ↔ `step_complete`).
Binary frames reserved for future use.

### Worker → coordinator

| type             | payload                                                                                          |
|------------------|--------------------------------------------------------------------------------------------------|
| `register`       | `{name, version, hw_caps, available_steps, plugin_manifest:[{name,version,sha256}]}`             |
| `heartbeat`      | `{}` — every 30s; coordinator times out worker after 90s of silence                              |
| `step_progress`  | `{run_id, step_id, kind:"log"|"pct"|"context_set", payload}` — mirrors existing `StepProgress`   |
| `step_complete`  | `{run_id, step_id, status:"ok"|"error", error?, ctx_diff:{steps:{...}}}`                         |
| `claim_capacity` | `{semaphores:{nvenc:{used,total}, vaapi:{used,total}, ...}}` — periodic; informs scheduler       |

### Coordinator → worker

| type                | payload                                                                                  |
|---------------------|------------------------------------------------------------------------------------------|
| `register_ack`      | `{worker_id, plugin_install:[{name,version,sha256,tarball_url}]}` — full intended state |
| `plugin_install`    | `{name, version, sha256, tarball_url}` — pushed on coordinator-side install/upgrade      |
| `plugin_uninstall`  | `{name}` — pushed on coordinator-side uninstall                                          |
| `step_dispatch`     | `{run_id, step_id, use, with, ctx, run_on?}` — ctx carries file/probe/prior step output |
| `cancel`            | `{run_id}` — kill any in-flight ffmpeg child for that run                                |

### Plugin tarball delivery

Coordinator exposes `GET /api/worker/plugins/:name/tarball` (authed
by worker token). Worker installer (existing
`crates/transcoderr/src/plugins/installer.rs`) fetches from there
instead of the catalog. Sha256 verification on the worker side stays
unchanged. Workers don't need their own catalog config.

## Dispatch algorithm (v1)

For each step the flow engine reaches:

1. Resolve target executor from step routing rules (built-in default
   → plugin manifest → flow YAML override).
2. If `coordinator`: run in-process (existing path).
3. If `any-worker`:
   a. Find connected workers that have the step in their
      `available_steps` AND have free per-device capacity per their
      last `claim_capacity`.
   b. If none available and local worker is enabled with capacity,
      fall back to local.
   c. If still none, mark step pending; resume when a worker
      connects or frees capacity.
   d. Pick one (v1: round-robin among eligible). Send
      `step_dispatch`.
   e. Await `step_complete` with a step-kind timeout (e.g. 6h for
      transcodes; 30s for probe). On timeout, treat as worker
      failure.

### Failure modes

- **Worker disconnect mid-step.** Coordinator marks the step as
  failed-with-retry; requeues to another worker (or local).
- **Heartbeat timeout (90s no traffic).** Connection dropped; same
  as above.
- **Worker reconnect with same token.** Gets the same `worker_id`
  row. Picks up where it left off only if it re-claims any in-flight
  step in its register payload.

## UI changes (additive, all on coordinator)

- **New top-level page: Workers.** Sidebar entry. Lists registered
  workers. Per-row: name, status (connected / stale / offline), kind
  (local / remote), hw caps summary (`NVENC ×3, VAAPI ×8`), current
  load (X of Y semaphores), version, current step (if any),
  connected-since timestamp. Inline "Disable" toggle for the local
  worker. "Generate token" button (one-time-display modal). "Delete"
  per remote worker (drops row + revokes token).
- **Settings → Workers section.** Same generate-token UI plus the
  local-worker enable/disable toggle. Workers page is operational
  view; Settings is configuration.
- **Run timeline shows which worker handled each step.** New
  `worker_name` field on step events; timeline reads `▶ plan.execute
  → started on gpu-box-1`. Backfills as `local` for existing runs.
- **Flow editor honors `run_on:`.** Validation in `flow/parser.rs`
  accepts `run_on: coordinator | any-worker`.

## Database additions

One migration:

```sql
CREATE TABLE workers (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,            -- 'local' | 'remote'
    secret_token TEXT,                     -- NULL for the local worker row
    hw_caps_json TEXT,                     -- last register payload
    plugin_manifest_json TEXT,             -- last register payload
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_seen_at INTEGER,                  -- unix seconds; NULL = never
    created_at   INTEGER NOT NULL
);

ALTER TABLE jobs       ADD COLUMN worker_id INTEGER REFERENCES workers(id);
ALTER TABLE run_events ADD COLUMN worker_id INTEGER;

INSERT INTO workers (name, kind, enabled, created_at)
VALUES ('local', 'local', 1, strftime('%s', 'now'));
```

## Decomposition

Each piece = one spec → plan → PR. Each piece independently
shippable; the project value compounds with each.

### Piece 1: Wire protocol skeleton + worker daemon

- `transcoderr worker` subcommand. WS dial → register → heartbeat
  loop. Bearer auth. Reconnect with backoff.
- DB migration for `workers` table; local-worker row seeded.
- Coordinator-side WS handler at `/api/worker/connect`. Validates
  token, accepts register, stores last hw caps + plugin manifest,
  updates `last_seen_at`.
- Workers UI page (read-only listing) + token generation flow.
- No dispatch yet. A worker connects, registers, heartbeats. That's
  it. Visible in the Workers UI.

### Piece 2: Local-worker abstraction

- Refactor the in-process worker pool so it registers through the
  same mechanism remote workers use. The local worker becomes "just
  another worker" with `kind=local`.
- Settings: enable/disable toggle for the local worker.
- Existing single-host behavior unchanged for users without remotes.
- Sets up the abstraction the next piece needs.

### Piece 3: Per-step routing + remote dispatch (built-ins only)

- Step kinds gain a default executor attribute.
- Flow YAML accepts `run_on:`; parser + validation.
- Dispatcher round-trips `step_dispatch` ↔ `step_complete` for
  built-in remote-eligible steps (`plan.execute`, `transcode`,
  `remux`, `extract.subs`, `iso.extract`, `audio.ensure`,
  `strip.tracks`).
- Plugin steps stay on coordinator for now (Piece 5 wires them).
- Run timeline gets `worker_name`.
- First end-to-end remote work happens here.

### Piece 4: Plugin push to workers

- Coordinator exposes `GET /api/worker/plugins/:name/tarball`.
- `register_ack` sends the full intended plugin manifest; worker
  installs missing ones via the existing pipeline pointed at the
  coordinator URL.
- `plugin_install` / `plugin_uninstall` deltas pushed over WS on
  coordinator-side changes.
- Workers' plugin dir stays independent; no shared mount required.

### Piece 5: Plugin steps remote-eligible

- Plugin manifests gain optional `executor = "any-worker"`.
- Routing rules respect plugin manifest defaults.
- Workers' subprocess plugin pipeline (already shipped) wired into
  the dispatch path.
- Heavy plugins (e.g. whisper) now offload-able.

### Piece 6: Failure handling + reassignment

- Heartbeat timeout → worker marked stale; in-flight steps
  reassigned.
- Worker disconnect mid-step → step fail-with-retry; pick another
  worker (or local).
- Cancel propagation: coordinator-side cancel → worker `cancel`
  message → ffmpeg child killed.
- Worker reconnect with same token: picks up in-flight work only if
  re-claims it in register payload.

## Out of scope (initial 6 pieces)

- **mDNS / auto-discovery.** Manual config only.
- **File shipping.** Shared filesystem assumption.
- **Best-fit scheduling.** Round-robin in v1; smarter scheduler is a
  future piece if real load patterns demand it.
- **Multi-coordinator / peer-to-peer.** Single coordinator forever.
- **Tenant isolation / per-flow worker pinning.** All workers see all
  jobs from the coordinator they're connected to.
- **Worker-side web UI.** Workers report via coordinator; no
  standalone UI.
- **Encrypted workspace separation.** Workers run as the same trust
  level as the coordinator (already true for plugins today).
- **Auto-upgrade.** Workers must run a compatible version of
  transcoderr; mismatched register payload closes the connection
  with a clear error. Operator handles upgrades.

## Risks

- **WebSocket reverse-proxy compatibility.** Some reverse proxies
  need explicit upgrade headers. Documented in the worker config
  notes.
- **Long-running ffmpeg jobs + WS idle.** Heartbeat every 30s keeps
  the connection live through proxy idle timeouts.
- **Plugin install drift on worker reconnect.** The
  `register_ack` carries the full intended manifest, so a worker
  that was offline through several plugin upgrades catches up
  on reconnect via diff vs its current install set.
- **Local worker race during refactor (Piece 2).** The existing
  worker pool is load-bearing for every install today. The
  refactor must preserve exact behavior on single-host deployments.
  Heavy testing on Piece 2 mandatory.
- **Backwards compat with existing flows.** Flows without `run_on:`
  must keep working — the parser must default `run_on:` to the
  step kind's built-in default.

## Success criteria (whole roadmap)

- An operator can add a remote GPU box to handle ffmpeg/heavy plugin
  work for an existing CPU-only transcoderr deployment with **zero
  changes to existing flow YAMLs**, via the Workers UI.
- Default behavior for users without remote workers is identical to
  today.
- Workers can disconnect/reconnect at will without losing pending
  jobs.
- Plugin install on the coordinator UI propagates automatically to
  all connected workers.
- The MCP tool surface gains read-only access to the workers
  registry (later piece, not core roadmap).

## Branch / PR

- Branch `spec/distributed-transcoding` for this roadmap doc.
- Each implementation piece gets its own `feat/<piece-name>`
  branch + PR.
- Subsequent pieces' specs live alongside this one in
  `docs/superpowers/specs/` and reference this roadmap as their
  parent.
