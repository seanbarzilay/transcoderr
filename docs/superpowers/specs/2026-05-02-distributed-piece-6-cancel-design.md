# Distributed Transcoding — Piece 6: Failure Handling + Reassignment (FINAL)

## Goal

Cancel signals propagate from the coordinator's UI all the way to the
remote worker's ffmpeg child process. Within 1 second of clicking
Cancel on a job whose current step is dispatched remote, the worker
receives a `StepCancel` envelope, kills its running subprocess, and
the coordinator records the run as `cancelled` (not as a 30-second
timeout failure).

This is the final piece of the original 6-piece distributed-
transcoding roadmap from issue #79. The spec is intentionally narrow:
**the engine's existing retry policy + the dispatcher's per-attempt
`route()` + Piece 3's 30s frame timeout already cover the
"fail-with-retry; pick another worker (or local)" semantics**. The
only meaningful new behavior is cancel propagation.

## Roadmap context

- Roadmap parent: `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md` (PR #81 merged).
- Piece 1: PR #83 / v0.32.0 — wire protocol + worker daemon.
- Piece 2: PR #85 / v0.33.0 — local-worker abstraction.
- Piece 3: PR #87 / v0.34.0 — per-step routing + remote dispatch (built-ins).
- Piece 4: PR #89 / v0.35.0 — plugin push to workers.
- Piece 5: PR #91 / v0.36.0 — plugin steps remote-eligible.
- This piece (Piece 6): cancel propagation. **Closes out issue #79.**

## Locked-in decisions (from brainstorming)

1. **Cancel: fire `StepCancel` and bail immediately** — coordinator
   sends the envelope, returns `Err("cancelled")` to the engine
   without waiting for the worker's ack. The engine records the run
   as cancelled fast; the worker eventually kills ffmpeg in the
   background.
2. **Idle-sweep stays observation-only** — the existing 30s frame
   timeout + UI's `last_seen_at` polling are sufficient for stale-
   worker handling. No bus events, no auto-cancellation by sweep.
3. **Reconnect-with-in-flight-work is deferred** — engine retry
   policy + per-step timeout cover the operational failure modes;
   re-attaching to a healthy ffmpeg after a clean WS bounce is a
   marginal optimisation that saves ~150 LoC + tricky tests for a
   problem the operators haven't reported. If it ever becomes
   painful, a future piece can ship it cleanly.

## What is already covered (no Piece 6 work needed)

- **Worker disconnect mid-step → fail-with-retry on another worker.**
  `RemoteRunner::run` returns `Err` on disconnect; the engine's
  existing retry loop re-runs the step; `dispatch::route` is called
  fresh on each attempt and naturally picks a different worker (or
  falls back to local).
- **Heartbeat timeout / stale workers.** `RemoteRunner`'s 30s frame
  timeout fails any in-flight remote step targeting a silent worker.
  The Workers UI already shows stale workers via the `last_seen_at`
  age check on its 15s React-Query refetch.
- **Reassignment.** Falls out for free from the retry-then-route-
  fresh path. No reassignment tracker is needed in Piece 6.

## Architecture

### Coordinator side: cancel-watching `RemoteRunner`

`RemoteRunner::run` already loops on the inbox channel with a 30s
frame timeout (Piece 3). Add a third `tokio::select!` arm that watches
`ctx.cancel.cancelled()`. When the operator clicks Cancel:

1. The existing API path calls `JobCancellations::cancel(job_id)`,
   which fires the `CancellationToken` threaded into `Context.cancel`.
2. `RemoteRunner`'s select arm wakes immediately.
3. The runner sends a `StepCancel` envelope to the worker via
   `Connections::send_to_worker`.
4. The runner returns `Err("step cancelled by operator")`.
5. The engine's existing per-step error path records the run as
   cancelled (same code path as a local-step cancel).

```rust
let cancel = ctx.cancel.clone();  // Option<CancellationToken>
loop {
    let frame = tokio::select! {
        f = tokio::time::timeout(STEP_FRAME_TIMEOUT, rx.recv()) => match f {
            Ok(Some(f)) => f,
            Ok(None) => anyhow::bail!("worker inbox channel closed"),
            Err(_) => anyhow::bail!("worker step timed out"),
        },
        _ = async {
            match &cancel {
                Some(c) => c.cancelled().await,
                None => std::future::pending().await,
            }
        } => {
            let env = Envelope {
                id: correlation_id.clone(),
                message: Message::StepCancel(StepCancelMsg {
                    job_id,
                    step_id: step_id.into(),
                }),
            };
            let _ = state.connections.send_to_worker(worker_id, env).await;
            anyhow::bail!("step cancelled by operator");
        }
    };
    // process frame as today (Progress / Complete arms unchanged)
}
```

The `std::future::pending()` arm makes the select branch never fire
when `ctx.cancel` is `None` (test fixtures, edge cases) — the loop
just behaves exactly as today. Backward compatible.

### Worker side: per-connection cancel registry

A new `step_cancellations: Arc<RwLock<HashMap<String,
CancellationToken>>>` lives in `worker/connection.rs::connect_once`
alongside the existing `sync_slot` and `outbound_tx`. Two changes
ripple out:

**`handle_step_dispatch` (worker/executor.rs)** gains the cancellation
parameter. On dispatch start: register a fresh token + attach to
`Context.cancel`. On step return: unregister.

```rust
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
    step_cancellations: Arc<RwLock<HashMap<String, CancellationToken>>>,
) {
    // existing snapshot parse + registry::resolve + with_map setup
    let mut ctx = Context::from_snapshot(&dispatch.ctx_snapshot)?;

    // NEW: register cancel token, attach to ctx.cancel.
    let token = CancellationToken::new();
    step_cancellations.write().await.insert(correlation_id.clone(), token.clone());
    ctx.cancel = Some(token);

    // existing on_progress callback setup
    // existing step.execute(&with_map, &mut ctx, &mut on_progress).await

    // NEW: unregister on completion (success OR failure).
    step_cancellations.write().await.remove(&correlation_id);

    // existing send_complete
}
```

Existing transcode + subprocess steps read `Context.cancel.cancelled()`
to abort their work — the cancel signal flows from
coordinator-`StepCancel` → worker token registry → `Context.cancel`
→ ffmpeg child SIGKILL. **Zero changes** to the step impls
themselves; the machinery is the same one Piece 3 already uses for
local-side cancellation.

**Receive loop arm (worker/connection.rs)** for the new envelope:

```rust
Message::StepCancel(p) => {
    let map = step_cancellations.read().await;
    if let Some(token) = map.get(&envelope.id) {
        token.cancel();
        tracing::info!(
            job_id = p.job_id,
            step_id = %p.step_id,
            correlation_id = %envelope.id,
            "step cancel received"
        );
    } else {
        tracing::debug!(
            correlation_id = %envelope.id,
            "step cancel for unknown correlation; dropped"
        );
    }
}
```

The `else` branch covers the race where StepCancel arrives after the
step already completed (the worker had already removed the entry).
Silent drop with a debug log.

### Wire protocol additions

One new `Message` variant in `worker/protocol.rs`:

```rust
StepCancel(StepCancelMsg),

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepCancelMsg {
    pub job_id: i64,
    pub step_id: String,
}
```

The envelope's existing `id` field carries the correlation_id (matches
the original `step_dispatch`). `job_id + step_id` are for the
worker's log context only.

## File structure

**Modified backend files:**
- `crates/transcoderr/src/worker/protocol.rs` — `StepCancel` variant
  + struct + 1 round-trip test.
- `crates/transcoderr/src/dispatch/remote.rs` — `RemoteRunner::run`
  adds the cancel-watching select arm.
- `crates/transcoderr/src/worker/executor.rs` —
  `handle_step_dispatch` gains the `step_cancellations` parameter,
  registers + unregisters per dispatch, attaches token to
  `Context.cancel`.
- `crates/transcoderr/src/worker/connection.rs` — sets up the per-
  connection `step_cancellations`; passes it to the spawned
  `handle_step_dispatch` task; receive loop's match arms gain the
  `Message::StepCancel(p)` handler.

**New backend files:**
- `crates/transcoderr/tests/cancel_remote.rs` — 3-scenario integration
  suite.

**No new files except the test. No DB migration. No new dependencies.**

## Wire / API summary

| Endpoint / Envelope | Direction | Purpose | Status |
|---|---|---|---|
| `POST /api/jobs/:id/cancel` (existing) | UI → coordinator | Operator triggers cancel | unchanged |
| `JobCancellations::cancel(job_id)` (existing) | coordinator-internal | Triggers `Context.cancel` token | unchanged |
| `StepCancel` envelope | coordinator → worker | NEW — propagates cancel mid-remote-step | **new** |

## Database

**No schema migration.** All state needed for Piece 6 lives in
existing structures: `JobCancellations` (in-memory map of job_id →
token, exists since v0.31), and a new in-memory per-connection
`step_cancellations` map (correlation_id → token) that lives for the
WS connection's lifetime.

## Failure scenarios

| Scenario | Behavior |
|---|---|
| Operator cancels a job whose current step is **local** | Existing path: token fires, transcode/etc. step kills ffmpeg, returns Err, engine records cancelled. **Unchanged.** |
| Operator cancels a job whose current step is **remote** | RemoteRunner's select arm sees the cancel; sends `StepCancel` to worker; bails with `Err("step cancelled by operator")`. Worker's StepCancel arm fires the token; running step's ffmpeg killed; eventual step_complete{failed} arrives at the (already-torn-down) inbox and is dropped silently. Engine records cancelled. |
| Operator cancels a job that is between steps | Token fires; engine's per-step retry loop sees `cancel_token.is_cancelled()` (existing local check); engine bails immediately without dispatching the next step. **Unchanged from today.** |
| Worker disconnects between coordinator's `StepCancel` send and its actual receipt | StepCancel envelope sits in the worker's outbound mpsc; on disconnect the channel drops; envelope discarded. Worker's ffmpeg child is now orphaned but the worker daemon's eventual death (SIGTERM, systemd restart, machine reboot) cleans up. Coordinator-side: RemoteRunner already bailed; engine already recorded cancelled. |
| Two cancels in quick succession (operator double-clicks) | Second `cancel()` call on an already-cancelled token is a no-op (CancellationToken semantics). The second StepCancel envelope arrives at the worker after the first cancel already removed the registry entry → silent debug-log drop. |
| `StepCancel` arrives at worker after `step_complete` already sent | Worker's `step_cancellations` map no longer has the entry (`unregister` fired on completion). The StepCancel arm logs at debug-level and drops silently. |
| `Context.cancel = None` on the worker (e.g. snapshot deserialise hadn't been patched) | The `step_cancellations.insert` runs unconditionally; `ctx.cancel = Some(token)` overwrites whatever from_snapshot returned. **The cancel registry is the source of truth for the token, not the snapshot.** Pre-existing fact (Piece 3 already overwrote `ctx.cancel` on the coordinator side; Piece 6 mirrors it on the worker side). |

## Testing

### Unit tests

- `worker/protocol.rs` — `StepCancel` JSON round-trip + lock the
  wire tag (`"type":"step_cancel"`).

### Integration tests (`crates/transcoderr/tests/cancel_remote.rs`)

Reuses `common::boot()` + the fake-worker harness from Piece 3's
`tests/remote_dispatch.rs`.

1. **`cancel_propagates_to_remote_worker`** — connect a fake worker
   that accepts `step_dispatch` and never replies. Submit a job with
   a `transcode` step (`run_on: any`). Wait for the dispatch to
   arrive at the fake worker. Trigger `JobCancellations::cancel(job_id)`
   directly via a test helper (or via the cancel API endpoint).
   Assert the fake worker receives a `StepCancel` envelope within
   1s. Assert the engine records the run as `cancelled` within 2s.
2. **`cancel_unblocks_engine_within_a_second`** — same fixture;
   measure wall-clock from the cancel call to the run reaching
   `cancelled` status. Assert it's under 2s. **Verifies the cancel-
   watching select arm fires before the 30s frame timeout.**
3. **`cancel_after_step_complete_is_silent`** — fake worker replies
   with `step_complete{ok}` immediately. Wait for the run to reach
   `completed`. THEN call cancel. Assert no `StepCancel` envelope
   arrives at the fake worker within a 1s wait window (the inbox
   guard tore down on step_complete; the cancel finds nothing in
   flight).

### Existing tests must stay green

- `worker_connect` (4) — initial register flow unchanged.
- `local_worker` (4) — no cancel propagation involved.
- `remote_dispatch` (5) — fake worker harness sends step_complete
  promptly; no cancel paths exercised.
- `plugin_push` (6) — no cancel work.
- `plugin_remote_dispatch` (5) — fake worker advertises step kinds;
  no cancel paths.
- `api_auth` (7), critical-path tests, full lib suite.

The new `Message::StepCancel(_)` arm in the worker's receive loop
coexists with the existing `step_dispatch` and `plugin_sync` arms.

## Risks

- **`Context.cancel` plumbing on the worker side.** The existing
  transcode + subprocess steps READ `ctx.cancel.cancelled()` to abort.
  After Piece 3, `Context::from_snapshot` deserialises with
  `cancel: None` (`#[serde(skip)]`). Piece 6's fix is mechanical:
  `handle_step_dispatch` sets `ctx.cancel = Some(token)` after parsing
  the snapshot. This is an additive change to the existing code path;
  no step impl changes needed.
- **Race between StepCancel send and worker disconnect.** If the
  worker disconnects mid-cancel, the StepCancel envelope sits in a
  dropped mpsc. The coordinator's `RemoteRunner` already returned
  Err by then (it bailed immediately on the cancel signal). Engine
  records cancelled. Worker's orphaned ffmpeg eventually dies on
  worker daemon shutdown. **Acceptable; matches today's "WS dropped
  mid-step" behavior.**
- **Per-connection registry leaks on panic.** `step_cancellations`
  is keyed by correlation_id (UUIDv4). Entries are removed on step
  completion. If a panic between register and unregister leaks an
  entry, the connection's drop cleans up the whole HashMap when the
  WS task exits. **No long-term leak.**
- **`Context.cancel.cancelled()` future-pinning subtlety in
  `RemoteRunner`.** The async block inside the select arm captures
  `&cancel` (a `&Option<CancellationToken>`). Each call to
  `c.cancelled()` returns a fresh future; using it inside a tokio
  select is supported by `tokio_util::sync::CancellationToken`. **Pre-
  existing pattern** (the coordinator's transcode step already does
  this). Mirroring it is mechanical.

## Out of scope (deferred from Piece 6)

- **Reconnect-with-in-flight-work re-attach** — Q3-A confirmed: defer
  to a future piece. ~150 LoC + complex tests; engine retry covers
  the operational failure mode.
- **Idle-sweep auto-cancellation of stale-worker in-flight steps** —
  Q2-A confirmed: existing 30s frame timeout + UI polling sufficient.
- **Bus-event-driven UI updates for stale workers** — UI's existing
  15s React-Query polling is fast enough.
- **Worker self-cancellation on long-running step** — workers don't
  have visibility into the operator's intent; cancel always flows
  coordinator → worker.
- **Cascading cancellation across a job's remaining steps** — already
  handled by the engine's existing retry loop's
  `cancel_token.is_cancelled()` check between steps. No new code
  needed.

## Out of scope (whole roadmap, recap from `docs/superpowers/specs/2026-05-02-distributed-transcoding-design.md`)

- mDNS / auto-discovery (manual config only).
- File shipping (shared filesystem assumption).
- Best-fit scheduling (round-robin in v1).
- Multi-coordinator / peer-to-peer (single coordinator forever).
- Tenant isolation / per-flow worker pinning.

## Success criteria

1. Operator clicks Cancel on a running job whose current step is
   dispatched to a remote worker.
2. Within 1 second, the worker receives a `StepCancel` envelope.
3. The worker's running ffmpeg child is killed via the existing
   `Context.cancel` machinery (no step-impl changes).
4. The coordinator's `RemoteRunner` returns immediately; engine
   records the run as `cancelled`.
5. The job's run-detail page shows the step as cancelled, not as a
   30-second timeout failure.
6. All existing integration tests stay green: `worker_connect` (4),
   `local_worker` (4), `remote_dispatch` (5), `plugin_push` (6),
   `plugin_remote_dispatch` (5), `api_auth` (7), critical-path
   tests, full lib suite.
7. **The original 6-piece distributed-transcoding roadmap from issue
   #79 is closed.**

## Branch / PR

Branch: `feat/distributed-piece-6` from main. Spec branch is
`spec/distributed-piece-6` (this file). Single PR per piece, matching
the Piece 1-5 pattern. **This closes out issue #79.**
