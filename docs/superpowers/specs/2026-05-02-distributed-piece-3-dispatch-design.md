# Distributed Transcoding — Piece 3: Per-step Routing + Remote Dispatch

## Goal

Make the first piece of distributed transcoding actually do remote work.
The flow engine routes individual steps to remote workers when the
step's kind is remote-eligible and a worker is connected. Step output
streams back as live progress; the run timeline shows which worker
executed each event. Plugin steps stay coordinator-only (Piece 5 wires
them); job reassignment on disconnect is Piece 6.

## Roadmap context

- Roadmap parent: `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md` (PR #81 merged).
- Piece 1 (wire protocol skeleton + worker daemon): merged in PR #83 as
  `v0.32.0`. The `workers` table, the seeded `local` row, the WS
  `register/register_ack/heartbeat` envelope, the REST CRUD, and the
  auth-middleware extension all exist.
- Piece 2 (local-worker abstraction): merged in PR #85 as `v0.33.0`.
  The local row registers like a remote and the per-row enable toggle
  is operational.
- This piece (Piece 3) is the **first piece where remote work actually
  happens**. After this, an operator with one connected worker can run
  a flow whose `transcode` step ffmpegs on the worker host.

## Locked-in decisions (from brainstorming)

1. **Dispatcher lives inline in `Engine::run_nodes`.** Right before the
   existing `step.execute(...)` call, the engine consults a router and
   either runs locally as today or hands off to a `RemoteRunner`. No
   separate background dispatcher; no per-job (whole-flow) routing.
2. **Wire protocol is by-value.** `step_dispatch` carries the full
   `{step_id, use, with, ctx_snapshot, job_id}` payload. Worker has
   everything it needs from the WS frame; no REST round-trips, no new
   auth surface.
3. **Routing model is registry trait + YAML override.** The `Step`
   trait gains `fn executor(&self) -> Executor { Executor::CoordinatorOnly }`
   with a default. The 7 built-ins listed in the roadmap override to
   `Executor::Any`. Flow YAML accepts `run_on: any | coordinator` per
   step. Specific-worker-name targeting is **out of scope**.
4. **Worker selection is round-robin among eligible+enabled+fresh
   workers.** Eligible = enabled=1, last_seen_at within 90s, step kind
   in `available_steps`. The local row is included if it covers the
   kind (it does — Piece 2's local register reports all built-ins).
   No hw_caps-aware selection (deferred), no least-loaded (deferred).
5. **Per-event worker attribution via `run_events.worker_id`.** Piece 1
   added this nullable column for exactly this purpose. Every
   `append_with_bus_and_spill` call grows a `worker_id` parameter. UI
   joins to `workers.name` for display.
6. **Failure on disconnect mid-step is just `Err` to the engine.**
   Engine's existing `retry:` policy applies; otherwise the run goes
   to `failed`. Reassignment is Piece 6.

## Architecture

### Routing (per step, before execute)

```text
┌──────────────────────────────────────────────────┐
│ Engine::run_nodes() loop                         │
│   for each Node::Step in flow:                   │
│     ─► dispatch::route(use_, run_on, &state)     │
│         => Route::Local | Route::Remote(worker_id)│
│     match route {                                │
│       Local         => step.execute(...)         │
│       Remote(wid)   => RemoteRunner::run(...)    │
│     }                                            │
└──────────────────────────────────────────────────┘
```

The branch lives at one place in `flow/engine.rs`; everything else
flows from there.

### `Step` trait extension

```rust
pub enum Executor { CoordinatorOnly, Any }

#[async_trait]
pub trait Step: Send + Sync {
    fn name(&self) -> &'static str;
    async fn execute(...) -> anyhow::Result<()>;

    /// Default: coordinator-only. Each remote-eligible built-in
    /// overrides this. Subprocess plugins keep the default until
    /// Piece 5 wires plugin push.
    fn executor(&self) -> Executor { Executor::CoordinatorOnly }
}
```

The 7 built-ins that flip to `Any`:
`plan.execute, transcode, remux, extract.subs, iso.extract,
audio.ensure, strip.tracks`.

### Flow YAML grammar

Per-step `run_on:` is optional. Allowed values: `any`, `coordinator`.
Anything else is a parse error.

```yaml
steps:
  - use: probe                 # default: CoordinatorOnly  → Local
  - use: transcode             # default: Any              → Remote (if a worker exists)
    run_on: any
  - use: transcode
    run_on: coordinator        # explicit override         → Local
  - use: notify
    run_on: any                # PARSE ERROR (CoordinatorOnly + run_on:any)
```

| `run_on` | step.executor()    | result                       |
|----------|--------------------|------------------------------|
| `"any"`  | `Any`              | Remote (round-robin)         |
| `"any"`  | `CoordinatorOnly`  | Parse error                  |
| `"coordinator"` | (anything)  | Local                        |
| absent   | `Any`              | Remote (round-robin)         |
| absent   | `CoordinatorOnly`  | Local                        |

### Wire protocol

Three new variants in `Message` (defined in `worker/protocol.rs`):

```rust
StepDispatch(StepDispatch),    // coordinator → worker
StepProgress(StepProgressMsg), // worker → coordinator
StepComplete(StepComplete),    // worker → coordinator
```

```rust
#[derive(Serialize, Deserialize)]
pub struct StepDispatch {
    pub job_id: i64,
    pub step_id: String,
    #[serde(rename = "use")]
    pub use_: String,
    pub with: serde_json::Value,
    pub ctx_snapshot: String,   // result of Context::to_snapshot()
}

#[derive(Serialize, Deserialize)]
pub struct StepProgressMsg {
    pub job_id: i64,
    pub step_id: String,
    pub kind: String,           // "progress" | "log" | marker.kind
    pub payload: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct StepComplete {
    pub job_id: i64,
    pub step_id: String,
    pub status: String,         // "ok" | "failed"
    pub error: Option<String>,  // present when status == "failed"
    pub ctx_snapshot: Option<String>, // present when status == "ok"
}
```

Envelope `id` field correlates a `step_dispatch` with its eventual
`step_complete`.

### Connection registry

`crates/transcoderr/src/worker/connections.rs` (new). Lives on
`AppState`:

```rust
pub struct Connections {
    senders: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    inbox:   Arc<RwLock<HashMap<String, mpsc::Sender<InboundStepEvent>>>>,
}
```

- `senders[worker_id]` → outbound channel that the WS handler's send
  task drains; `RemoteRunner::send_dispatch` looks up the worker's
  channel by id.
- `inbox[correlation_id]` → channel for `step_progress` / `step_complete`
  frames belonging to a specific dispatch. The WS receive loop demuxes
  inbound frames by `id` and forwards. Each `RemoteRunner` instance
  registers its correlation_id, awaits frames, and removes its entry
  on completion.

WS handler insert/remove paths use a small `ConnectionGuard` whose
`Drop` impl clears the registry — guarantees no leaks on panic.

### `RemoteRunner` (coordinator-side)

```rust
pub async fn run(
    state: &AppState,
    worker_id: i64,
    step_id: &str,
    job_id: i64,
    use_: &str,
    with: &serde_json::Value,
    ctx: &mut Context,
    on_progress: &mut (dyn FnMut(StepProgress) + Send),
) -> anyhow::Result<()>;
```

Behavior:
1. Build a `step_dispatch` envelope; mint UUIDv4 correlation id.
2. Register an inbox channel in `Connections` keyed by that id.
3. Send the envelope through the worker's outbound channel. If the
   worker has no channel (race: just disconnected), return
   `Err("worker disconnected")`.
4. Loop on the inbox channel:
   - `StepProgress` → translate to `StepProgress::{Pct,Log,Marker}` and
     call `on_progress(...)` so the engine's existing run_events
     pipeline fires.
   - `StepComplete{ok}` → restore `ctx` from `ctx_snapshot`; return
     `Ok(())`.
   - `StepComplete{failed}` → return `Err(error)`.
   - 30s no-frame timeout → return `Err("worker step timed out")`.
5. On any return path, remove the inbox entry (RAII guard).

### Worker-side executor

`crates/transcoderr/src/worker/executor.rs` (new). The worker's
existing `connection::run` receive loop currently silently drops
non-`Heartbeat` frames; this piece wires the new variants.

```rust
async fn handle_step_dispatch(
    tx: &mpsc::Sender<WsMessage>,
    correlation_id: String,
    dispatch: StepDispatch,
);
```

The worker side needs no pool or `AppState`: `registry::resolve` reads
the global `OnceCell` registry that Piece 1's worker daemon already
initialises at boot, and `Step::execute` carries any state it needs
inside `&self`.

Steps:
1. Parse `dispatch.ctx_snapshot` → `Context::from_snapshot`.
2. `registry::resolve(&dispatch.use_)` — same registry the worker's
   own (currently no-op) pool would use. The worker's daemon path
   needs to call `registry::init(...)` at boot for this to work; this
   is a small **Piece 3 wiring change** to `worker/daemon.rs`.
3. Build an `on_progress` callback that serialises each
   `StepProgress` event into a `step_progress` envelope (re-using
   the dispatch's `correlation_id`) and sends it on `tx`.
4. `step.execute(&dispatch.with, &mut ctx, &mut on_progress).await`.
5. Send a final `step_complete` envelope:
   - on `Ok` → `{status:"ok", ctx_snapshot: ctx.to_snapshot()}`
   - on `Err` → `{status:"failed", error: Some(e.to_string())}`

The worker is essentially running the same `Step::execute(...)` call
the local pool would have run; the difference is just where the bytes
land on disk (assumed to be a shared filesystem per the roadmap).

### Run timeline attribution

`db::run_events::append_with_bus_and_spill` gets a new
`worker_id: Option<i64>` parameter. Existing call sites in
`flow/engine.rs` thread it from a per-step `current_worker_id`
captured during routing:

```rust
let worker_id_for_event = match route {
    Route::Local         => Some(LOCAL_WORKER_ID),
    Route::Remote(wid)   => Some(wid),
};
```

`jobs.worker_id` is also stamped at first dispatch (the Engine sets it
once per job to the worker that ran the *first* remote step, or to
`LOCAL_WORKER_ID` if all steps were local). UI uses `jobs.worker_id`
for the run-row badge ("primary executor") and `run_events.worker_id`
for per-event attribution.

UI tweak in `web/src/pages/run-detail.tsx`: each event row gets a
small `[worker: <name>]` badge. Local events show `[worker: local]`.

## File structure

**New backend files:**
- `crates/transcoderr/src/dispatch/mod.rs` — `Route`, `Executor`-aware
  `route()` function, round-robin pointer (in-memory `AtomicUsize`).
- `crates/transcoderr/src/dispatch/remote.rs` — `RemoteRunner`.
- `crates/transcoderr/src/worker/connections.rs` — `Connections`
  registry + `ConnectionGuard` RAII helper.
- `crates/transcoderr/src/worker/executor.rs` — worker-side step
  dispatcher.
- `crates/transcoderr/tests/remote_dispatch.rs` — integration tests
  with a fake-worker harness.

**Modified backend files:**
- `crates/transcoderr/src/steps/mod.rs` — add `Executor` enum + default
  `executor()` method on `Step` trait.
- `crates/transcoderr/src/steps/transcode.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/remux.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/extract_subs.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/iso_extract.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/audio_ensure.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/strip_tracks.rs` — `executor() = Any`.
- `crates/transcoderr/src/steps/plan_execute.rs` — `executor() = Any`.
- `crates/transcoderr/src/worker/protocol.rs` — 3 new message types,
  3 new round-trip tests.
- `crates/transcoderr/src/flow/model.rs` — `Node::Step` gains
  `run_on: Option<RunOn>`.
- `crates/transcoderr/src/flow/parser.rs` — accept + validate `run_on:`,
  reject unknown values, reject `run_on: any` against
  CoordinatorOnly steps.
- `crates/transcoderr/src/flow/engine.rs` — call `dispatch::route`,
  branch on `Route::Local | Remote`, thread `worker_id` through the
  on_progress callback closure into run_events.
- `crates/transcoderr/src/db/run_events.rs` — add `worker_id` parameter
  to `append_with_bus_and_spill` (and any sibling helpers).
- `crates/transcoderr/src/db/jobs.rs` — `set_worker_id(pool, job_id,
  worker_id)`.
- `crates/transcoderr/src/api/workers.rs::handle_connection` — wire
  `Connections::insert` on register_ack, RAII removal on disconnect,
  inbound demux of `step_progress`/`step_complete` frames.
- `crates/transcoderr/src/http.rs` (or wherever `AppState` is defined)
  — add `connections: Arc<Connections>` field.
- `crates/transcoderr/src/main.rs` — construct `Connections::new()` at
  boot, plumb into `AppState`.
- `crates/transcoderr/src/worker/daemon.rs` — at boot, call
  `crate::steps::registry::init(...)` (with the worker's own pool —
  the worker daemon doesn't currently open a sqlite pool, so this
  becomes part of Piece 3 wiring: open an in-memory or temp-file
  `SqlitePool` for the worker process). The registry needs a pool
  for some built-in steps that consult settings / cache rows. If a
  step requires settings the worker can't satisfy, that's a Piece 5
  shape question; for the 7 remote-eligible built-ins, the pool is
  needed only by `transcode` (writes a checkpoint blob — verify
  during implementation; if not actually used, skip the pool open).
- `crates/transcoderr/src/worker/connection.rs` — the existing receive
  loop currently silently drops non-Heartbeat frames. Wire it to
  call `executor::handle_step_dispatch` on `Message::StepDispatch`
  and ignore `StepProgress`/`StepComplete` (those are
  worker→coordinator only).
- `crates/transcoderr/tests/common/mod.rs` — `boot()` constructs
  `Connections` for `AppState`, matching the production path.

**Modified web files:**
- `web/src/pages/run-detail.tsx` — render `[worker: <name>]` badge
  per event.
- `web/src/types.ts` — extend `RunEvent` with `worker_id: number | null`
  and `worker_name: string | null` (or whatever the API surfaces).
- `web/src/api/client.ts` — if the run-events endpoint shape changes,
  reflect here.

**No DB migration:** `jobs.worker_id` and `run_events.worker_id` were
added in Piece 1.

## Wire / API additions

### REST: `GET /api/runs/:id` (existing, response shape extended)

Each event in the response array gains `worker_id` and `worker_name`
(joined from `workers`). Consumers that ignore unknown fields are
unaffected. Backwards-compatible.

### WS: 3 new envelope types

Per the protocol section above. The existing register/heartbeat path
is unchanged.

## Database

No schema migration. All required columns exist from Piece 1:

- `jobs.worker_id INTEGER NULL` — stamped once per job at first
  dispatch.
- `run_events.worker_id INTEGER NULL` — stamped per event by
  `append_with_bus_and_spill`.

GET endpoints will need their JOIN: `SELECT ..., w.name AS worker_name
FROM run_events r LEFT JOIN workers w ON w.id = r.worker_id`.

## Failure semantics

| Scenario | Coordinator behavior |
|---|---|
| Worker WS drops mid-step (no `step_complete`) | 30s no-frame timeout in `RemoteRunner` → `Err("worker step timed out")` → engine retry policy applies → otherwise run fails |
| Worker reports `step_complete{failed, error}` | `Err(error)` → engine retry → otherwise run fails |
| Worker sends malformed JSON | WS receive loop logs warn + closes the connection; the dispatched step times out and fails as above |
| Coordinator can't find any eligible worker for an `Any` step | `route()` falls back to `Route::Local`, logs warn. **Better to run than fail.** |
| Two workers connect with the same token concurrently | Connections registry maps `worker_id → Sender`, so the second connect overwrites the first. The first's WS task sees `tx.send` errors and exits. (This is a degenerate case; operators don't share tokens. Documented but not engineered.) |

## Testing

### Unit tests

- `dispatch::route` — for each combination of (step.executor, run_on,
  eligible workers list shape), assert the right `Route`. Tests for:
  no eligible workers → Local fallback, all eligible disabled → Local,
  one eligible enabled+fresh → Remote, two eligible → round-robin
  alternates.
- `flow::parser` — round-trip `run_on: any` and `run_on: coordinator`;
  reject `run_on: nope`; reject `run_on: any` on a known
  CoordinatorOnly step.
- `worker::protocol` — JSON round-trip for `StepDispatch`,
  `StepProgressMsg`, `StepComplete`. The existing 4 tests still pass.

### Integration `tests/remote_dispatch.rs`

Reuses `common::boot()`. The fake worker is a small in-test
WebSocket client wired into `tokio_tungstenite`:

1. **`step_dispatched_to_remote_worker_completes`** — connect a fake
   worker that auto-replies `step_complete{ok}`; submit a flow with a
   `run_on: any` step that resolves to a no-op trivially-succeeding
   step in the fake worker; assert the run reaches `completed`.
2. **`progress_events_flow_back_to_run_events`** — fake worker sends
   2× `step_progress` then `step_complete{ok}`; assert run_events for
   that job include both progress events with `worker_id` matching
   the fake worker's row.
3. **`disconnect_mid_step_fails_run`** — fake worker accepts
   `step_dispatch`, then closes the connection without replying;
   assert run reaches `failed` within 30s + an "worker step timed
   out" error event lands in run_events.
4. **`coordinator_only_step_runs_locally`** — submit a `notify` step
   (CoordinatorOnly) while a fake worker is connected; assert no
   `step_dispatch` is sent to the worker.
5. **`no_eligible_workers_falls_back_to_local`** — disable all
   remotes via PATCH /api/workers/:id; submit a `transcode` step;
   assert it runs locally with a warn log.

### Existing tests must stay green

The Piece 1 + Piece 2 integration tests (`worker_connect.rs` 4
scenarios, `local_worker.rs` 4 scenarios, `api_auth.rs` 7), the
critical-path tests (`concurrent_claim`, `crash_recovery`,
`flow_engine`), and the full lib suite all continue to pass.

## Risks

- **`Step::execute` mutates ctx; ctx travels back over the wire.**
  Two-way ctx flow is the most subtle part. Test 2 specifically
  exercises this — the fake worker mutates ctx and we verify the
  mutation lands in the next step's view. If `Context::from_snapshot`
  is brittle (skipped fields, ordering deps), this test catches it.
- **Worker-side `registry::resolve` requires the registry to be
  initialised on the worker process.** Piece 1's worker daemon
  already calls `registry::init`, but it does so with empty plugins
  and a default `DeviceRegistry`. The 7 remote-eligible built-ins
  don't need `DeviceRegistry` for correctness in tests; in production
  the worker's hw registry should match its hardware. This is
  acceptable for Piece 3 (the test environment uses
  `HwCaps::default()`); Piece 5 will refine it.
- **Connection registry leaks on panic.** Mitigated by RAII
  `ConnectionGuard` in the WS handler; if the handler task panics,
  drop runs and removes the entry.
- **Correlation-id collisions.** UUIDv4 minted per dispatch; the
  birthday-bound is 2^61 dispatches before a 50/50 collision —
  effectively never.
- **The fake worker test harness is non-trivial.** Adds ~80-100 LOC
  of WS scripting helpers; risk is mostly bug-in-test-not-in-prod. We
  accept this because it's the right way to exercise the wire path
  end-to-end.

## Out of scope (initial 6 pieces context)

- **Plugin steps remote-eligible** — Piece 5 (needs Piece 4's plugin
  push first; subprocess plugins must run from the same artifacts).
- **Job reassignment on worker disconnect** — Piece 6.
- **Specific worker-name targeting** in YAML (`run_on: gpu-box-1`).
  Not blocked here; just deferred until operators ask for it.
- **hw_caps-based worker selection** — meaningful only after the
  hw_caps payload shape is real (Piece 5+).
- **Job-level routing** (whole-flow assigned to a worker). Per-step
  is the spec; coarser routing is an explicit non-goal.
- **Worker-side `cancel_job` propagation.** If the operator cancels
  a job while a remote step is in flight, Piece 3 still has the
  existing `cancellations` machinery only on the coordinator side;
  the cancel signal will fire when the engine *next* iterates, but
  it doesn't propagate to the worker mid-step. (Cancellations of
  in-flight remote work is a Piece 6 concern alongside reassignment.)

## Success criteria

1. With one remote worker connected (e.g. `transcoderr worker --config
   worker.toml`), running any flow that has a `transcode` step
   results in ffmpeg running on the worker host. The output file
   appears at the same shared-filesystem path the coordinator
   would have written to.
2. The run-detail timeline shows `[worker: gpu-box-1]` against the
   `transcode` events and `[worker: local]` against the surrounding
   `probe`, `output:replace`, and `notify` events.
3. Toggling the worker off via PATCH /api/workers/:id results in
   subsequent `run_on: any` steps falling back to local execution
   (with a warn log).
4. Disconnecting the worker mid-step results in the run failing
   within 30s with a "worker step timed out" error.
5. A flow with `notify` and `output:replace` steps (both
   CoordinatorOnly) runs entirely on the coordinator even when a
   worker is connected.
6. All Piece 1 + Piece 2 integration tests stay green; the new
   `tests/remote_dispatch.rs` 5 scenarios pass.

## Branch / PR

Branch: `feat/distributed-piece-3` from main. Spec branch is
`spec/distributed-piece-3` (this file). Single PR per piece, matching
the Piece 1/2 pattern.
