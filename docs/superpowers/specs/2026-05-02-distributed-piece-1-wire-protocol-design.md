# Distributed Transcoding — Piece 1: Wire Protocol Skeleton + Worker Daemon

**Date:** 2026-05-02
**Status:** Draft, pending implementation plan
**Parent roadmap:** `2026-05-02-distributed-transcoding-design.md`
**Issue:** #79

## Goal

Ship the connection layer end to end with no actual dispatch. After
this PR, an operator can:

1. Generate a worker token in the coordinator's UI.
2. Run `transcoderr worker --config worker.toml` on a separate host.
3. See the worker appear in the coordinator's Workers UI page with
   its hw caps and last-seen timestamp.
4. Watch the coordinator detect the worker as stale if it
   disconnects.

No jobs route to remote workers yet. The local in-process worker pool
keeps doing all the work. This piece is the foundation Pieces 2-6
build on.

## Decisions inherited from the roadmap

- One binary, new `transcoderr worker` subcommand.
- WebSocket wire protocol; worker dials coordinator.
- Bearer auth via per-worker token.
- Manual config (URL + token).
- DB additions: `workers` table; `jobs.worker_id`;
  `run_events.worker_id`. Local-worker row seeded by migration.
- Reconnect with exponential backoff.

## What this piece ships

### Backend

**New CLI subcommand** in `crates/transcoderr/src/main.rs`:

```rust
enum Cmd {
    Serve { config: PathBuf },
    Worker { config: PathBuf },     // NEW
}
```

`transcoderr worker --config worker.toml` reads:

```toml
coordinator_url   = "wss://coord.example/api/worker/connect"
coordinator_token = "wkr_xxx..."
name              = "gpu-box-1"      # optional; defaults to hostname
```

The worker daemon (new module `crates/transcoderr/src/worker/`):

- Probes hw caps once at boot (reuses existing
  `crate::hw::probe::probe()`).
- Inspects local plugin install set (reuses `crate::plugins::discover`).
- Opens a WebSocket to `coordinator_url` with `Authorization:
  Bearer <token>`.
- Sends `register` immediately on connect.
- Sends `heartbeat` every 30s.
- Logs every received frame at info level (no dispatch handling
  yet — log + drop).
- On disconnect: reconnects with backoff (1s → 2s → 4s → … → 30s
  capped). Each successful reconnect resets the backoff.

**Coordinator-side:**

- New module `crates/transcoderr/src/api/workers.rs` holds both
  the WS upgrade handler and the REST CRUD for the `workers`
  table (matches the existing `api/sources.rs` / `api/notifiers.rs`
  pattern of one file per resource).
- New route `GET /api/worker/connect` upgrades to WebSocket
  (axum's `WebSocketUpgrade`). Authed via the existing API-token
  middleware extended to also accept worker tokens (kept in the
  `workers` table's `secret_token` column).
- On connect: validates token → loads the matching `workers` row →
  awaits `register` frame within 5s → updates the row with
  `hw_caps_json`, `plugin_manifest_json`, `last_seen_at` → sends
  `register_ack` (with empty `plugin_install` list for now;
  Piece 4 fills it).
- Heartbeat handler: updates `last_seen_at` on each `heartbeat`
  frame.
- Idle timer: every 60s, sweeps `workers` rows where
  `last_seen_at < now - 90s` and pushes a "stale" event onto the
  existing SSE bus so the Workers UI updates live.

**Wire envelope** (this piece's subset):

```json
{ "type": "register",   "id": "<uuid>", "payload": {...} }
{ "type": "register_ack", "id": "<same-uuid>", "payload": {...} }
{ "type": "heartbeat",  "id": "<uuid>", "payload": {} }
```

`payload` for `register` (what Piece 1 needs):

```json
{
  "name": "gpu-box-1",
  "version": "0.31.0",
  "hw_caps": {
    "ffmpeg_version": "8.1",
    "encoders": ["h264_nvenc", "hevc_nvenc"],
    "devices": [
      {"accel": "nvenc", "index": 0, "name": "h264_nvenc (default)", "max_concurrent": 3}
    ]
  },
  "available_steps": ["plan.execute", "remux", "transcode"],
  "plugin_manifest": [
    {"name": "size-report", "version": "0.1.2", "sha256": "..."}
  ]
}
```

Coordinator stores `hw_caps` as `hw_caps_json` and the manifest as
`plugin_manifest_json`. `available_steps` is collapsed into
`hw_caps_json` for now (single column rather than a separate one;
Piece 3 will move it).

### Database

One migration:

```sql
CREATE TABLE workers (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,            -- 'local' | 'remote'
    secret_token TEXT,                     -- NULL for the local worker row
    hw_caps_json TEXT,
    plugin_manifest_json TEXT,
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_seen_at INTEGER,
    created_at   INTEGER NOT NULL
);

ALTER TABLE jobs       ADD COLUMN worker_id INTEGER REFERENCES workers(id);
ALTER TABLE run_events ADD COLUMN worker_id INTEGER;

INSERT INTO workers (name, kind, enabled, created_at)
VALUES ('local', 'local', 1, strftime('%s', 'now'));
```

`jobs.worker_id` and `run_events.worker_id` are nullable. Piece 1
doesn't write to them — they're added now so Piece 2's local-worker
refactor doesn't need a fresh migration.

### Coordinator API additions

| Method | Path                          | Purpose                                                |
|--------|-------------------------------|--------------------------------------------------------|
| GET    | `/api/workers`                | List rows in `workers` table (read-only)               |
| POST   | `/api/workers`                | Mint a new remote worker (returns one-time token)      |
| DELETE | `/api/workers/:id`            | Remove a remote worker (revokes token; row deleted)    |
| GET    | `/api/worker/connect`         | WebSocket upgrade — worker side                        |

`/api/workers` (read) returns `secret_token` redacted to `***` for
non-admin tokens, mirroring the existing sources/notifiers pattern.

### Web UI

New top-level page `web/src/pages/workers.tsx`:

- Sidebar entry between Plugins and Settings.
- Read-only table:
  | Status | Name | Kind | hw caps summary | Last seen | Actions |
- `Status`: connected / stale / offline (computed from
  `last_seen_at` vs now).
- Hw caps summary: small inline render — e.g. `NVENC ×3, VAAPI ×8`.
- Actions on remote rows: Delete (with confirm). No actions on the
  local row.
- "Add worker" button at top of the page → modal that mints a token
  via `POST /api/workers`, displays it once with a copy button +
  the worker config snippet (URL + token), warns the operator that
  the token won't be shown again.

The wizard (`web/src/components/setup-wizard.tsx`) is unchanged.

The local worker still does all the actual work — the UI just
displays it alongside the new remote rows. Piece 2 wires it through
the registration mechanism.

### MCP

Out of scope for Piece 1. MCP tools to list/manage workers can come
later.

## Files

### Module layout reorganization

`crates/transcoderr/src/worker.rs` exists today as the in-process
job-claim pool. Adding `crates/transcoderr/src/worker/mod.rs` would
be a Rust module-resolution conflict, so Piece 1 promotes the file
into a directory:

- `crates/transcoderr/src/worker.rs` → `crates/transcoderr/src/worker/pool.rs`
  (verbatim move; behaviour unchanged).
- New `crates/transcoderr/src/worker/mod.rs` re-exports `pool::*`
  for backwards compat with existing `use crate::worker::Worker`.
- New daemon files live as siblings under `worker/`.

Piece 2's refactor (local-worker registration) builds on this layout
without further moves.

**New:**
- `crates/transcoderr/migrations/<YYYYMMDDHHMMSS>_workers.sql` —
  filename uses the existing migration naming pattern (the
  implementer picks the actual datestamp).
- `crates/transcoderr/src/worker/mod.rs` — re-exports + entry for
  the daemon's `run` function
- `crates/transcoderr/src/worker/daemon.rs` — daemon orchestration:
  hw probe, plugin discover, dial, register, heartbeat loop
- `crates/transcoderr/src/worker/connection.rs` — WS client +
  reconnect-with-backoff loop
- `crates/transcoderr/src/worker/protocol.rs` — message types,
  shared between worker and coordinator (so the coordinator
  imports `crate::worker::protocol::*`)
- `crates/transcoderr/src/api/workers.rs` — both REST endpoints
  (`/api/workers`) AND the WS handler (`/api/worker/connect`).
  Single file per the existing `api/sources.rs` / `api/notifiers.rs`
  pattern.
- `crates/transcoderr/src/db/workers.rs` — CRUD for the `workers`
  table
- `web/src/pages/workers.tsx` — Workers UI page
- `web/src/components/forms/add-worker.tsx` — token-mint modal
- `crates/transcoderr/tests/worker_connect.rs` — integration test
  exercising the round-trip

**Renamed (verbatim move, no content change):**
- `crates/transcoderr/src/worker.rs` → `crates/transcoderr/src/worker/pool.rs`

**Modified:**
- `crates/transcoderr/src/main.rs` — new `Worker` subcommand;
  existing `use transcoderr::worker::Worker` still resolves via
  the new `worker/mod.rs` re-export
- `crates/transcoderr/src/api/mod.rs` — register the new routes
- `crates/transcoderr/src/lib.rs` — `pub mod worker` already exists;
  layout change is invisible to callers
- `web/src/App.tsx` — add the `/workers` route
- `web/src/components/sidebar.tsx` — sidebar link
- `web/src/api/client.ts` — typed wrappers for the new endpoints

## Wire dependencies

- Server: `axum::extract::ws::WebSocketUpgrade` (already present in
  axum 0.7; no new dep).
- Client: needs a WebSocket client. Two reasonable choices:
  - `tokio-tungstenite = "0.24"` — mature, async, ~tiny.
  - `reqwest-websocket` — wraps reqwest for WS, but reqwest's
    websocket feature isn't on by default in this codebase.
  Pick: **`tokio-tungstenite`**. Direct, well-known, no surprise
  feature flags.

## Tests

### Integration (wiremock-style with axum::Router::call)

`crates/transcoderr/tests/worker_connect.rs`:

1. `connect_with_valid_token_succeeds_and_register_persists` — mint
   a worker token via `POST /api/workers`; open a real WS to the
   in-process router with that token; send `register`; assert the
   `register_ack` comes back; query the DB and confirm `last_seen_at`
   + `hw_caps_json` are populated.
2. `connect_with_invalid_token_returns_401` — same flow with a junk
   token; expect a clean close with the right close code.
3. `heartbeat_keeps_last_seen_fresh` — connect, register, send 3
   heartbeats over 1s of test time, assert `last_seen_at` advances.
4. `idle_sweep_marks_stale_after_90s` — connect, register, drop the
   socket, advance the test clock past 90s, run the sweep,
   confirm the worker is no longer in the "connected" set surfaced
   by the SSE bus.

### Unit

- `worker/protocol.rs` — JSON round-trip tests for each message
  variant.
- `worker/connection.rs` — reconnect-with-backoff schedule check.
- `db/workers.rs` — CRUD smoke tests against an in-memory pool.

### Manual

- Build the binary; run two instances on the same host (different
  ports + different data dirs); coordinator on `:8080`, worker on
  `:8081`. Coordinator's UI shows the worker.
- Stop the worker; UI marks it stale within ~90s.

## Out of scope for Piece 1

- **Step dispatch.** Workers can register and heartbeat but receive
  no jobs. Piece 3 wires this up.
- **Local-worker registration through the same path.** Piece 2.
- **Plugin push.** `register_ack` returns an empty `plugin_install`
  list. Piece 4.
- **Failure-driven reassignment.** Stale detection only powers the
  UI status; jobs (still all run on the local in-process pool) are
  unaffected. Piece 6.
- **MCP tools** for workers.
- **mDNS / auto-discovery.** Operator types `coordinator_url` into
  the worker config.
- **Encrypted/dedicated transport.** WSS recommended in the docs;
  plain `ws://` accepted for ops who terminate TLS at a reverse
  proxy.

## Risks

- **WebSocket through reverse proxies.** Some proxy configs need
  explicit `Upgrade` headers passed through. Worth a one-line
  troubleshooting note in `docs/deploy.md`.
- **Token storage.** Worker token sits in plaintext in the worker's
  config file. Same threat model as the *arr API keys today; the
  config file is on a host the operator controls.
- **Auth-token middleware coupling.** The existing middleware checks
  the `api_tokens` table. Worker tokens live in `workers.secret_token`.
  The middleware needs a small extension to accept either; Piece 1
  must not break existing API token auth. Tested by keeping the
  existing `auth_*` tests green AND adding the new auth path.

## Success criteria

- A worker process can connect to a coordinator over WebSocket,
  register, and heartbeat.
- The coordinator's Workers UI shows the worker as connected with
  its hw caps.
- The worker appears stale within ~90s of disconnecting.
- Existing single-host transcoderr behaviour is unchanged. The
  full test suite passes (`cargo test -p transcoderr` +
  `npm --prefix web run build`).
- The `transcoderr worker` daemon doesn't accept any jobs yet —
  its dispatch surface is intentionally empty.
