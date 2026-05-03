# Distributed Transcoding — Piece 6 Implementation Plan (FINAL PIECE)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cancel signals propagate from the coordinator's UI all the way to the remote worker's ffmpeg child process, within ~1 second of the operator clicking Cancel.

**Architecture:** New `Message::StepCancel` envelope plus a per-connection `step_cancellations: HashMap<correlation_id, CancellationToken>` registry on the worker side. Coordinator's `RemoteRunner::run` adds a `tokio::select!` arm watching `ctx.cancel.cancelled()`; on cancel it sends `StepCancel` and bails immediately (fire-and-forget). Worker's receive loop fires the matching token; existing transcode/subprocess steps' `Context.cancel`-watching machinery handles the actual ffmpeg kill — no step impl changes.

**Tech Stack:** Rust 2021 (axum 0.7, sqlx + sqlite, tokio + `tokio_util::sync::CancellationToken`, anyhow, tracing). No new dependencies. **Smallest piece in the roadmap — closes out issue #79.**

**Branch:** all tasks land on a fresh `feat/distributed-piece-6` branch off `main`. Implementer creates the branch before Task 1.

---

## File Structure

**Modified backend files:**
- `crates/transcoderr/src/worker/protocol.rs` — new `StepCancel(StepCancelMsg)` variant + struct + 1 round-trip test.
- `crates/transcoderr/src/worker/connection.rs` — per-connection `step_cancellations` Arc, threaded into the spawned `handle_step_dispatch` task; new `Message::StepCancel(p)` arm in the receive loop.
- `crates/transcoderr/src/worker/executor.rs` — `handle_step_dispatch` gains a `step_cancellations` parameter; registers a fresh token at dispatch start, attaches to `Context.cancel` (overwriting the None from snapshot deserialise), unregisters on completion.
- `crates/transcoderr/src/dispatch/remote.rs` — `RemoteRunner::run` adds a `tokio::select!` arm watching `ctx.cancel.cancelled()`; on cancel sends `StepCancel` and returns `Err`.

**New backend files:**
- `crates/transcoderr/tests/cancel_remote.rs` — 3-scenario integration suite.

**No DB migration. No new dependencies.**

---

## Task 1: Wire protocol — `StepCancel` variant

Mechanical: one new message variant + struct + 1 round-trip test.

**Files:**
- Modify: `crates/transcoderr/src/worker/protocol.rs`

- [ ] **Step 1: Branch verification**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Update the `Message` enum**

In `crates/transcoderr/src/worker/protocol.rs`, find the existing `pub enum Message { ... }` block. It currently has 7 variants (Register, RegisterAck, Heartbeat, StepDispatch, StepProgress, StepComplete, PluginSync). Add `StepCancel`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum Message {
    Register(Register),
    RegisterAck(RegisterAck),
    Heartbeat(Heartbeat),
    StepDispatch(StepDispatch),
    StepProgress(StepProgressMsg),
    StepComplete(StepComplete),
    PluginSync(PluginSync),
    StepCancel(StepCancelMsg),
}
```

- [ ] **Step 3: Add the `StepCancelMsg` struct**

After the existing `StepComplete` struct (or anywhere near the other message structs), append:

```rust
/// Coordinator → worker. Tells the worker to abort the in-flight
/// step identified by the envelope's `id` (correlation_id, matching
/// the original `StepDispatch`). Worker side fires the registered
/// `CancellationToken` for that correlation, which propagates
/// through `Context.cancel` to running steps (kills ffmpeg etc.).
///
/// `job_id` and `step_id` are for log context on the worker side;
/// the correlation_id (envelope.id) is the actual lookup key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepCancelMsg {
    pub job_id: i64,
    pub step_id: String,
}
```

- [ ] **Step 4: Add a round-trip test**

In the existing `mod tests { ... }` block, append:

```rust
    #[test]
    fn step_cancel_round_trips() {
        let env = Envelope {
            id: "dsp-abc".into(),
            message: Message::StepCancel(StepCancelMsg {
                job_id: 42,
                step_id: "transcode_0".into(),
            }),
        };
        assert_eq!(round_trip(&env), env);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"step_cancel""#), "snake_case tag: {s}");
    }
```

- [ ] **Step 5: Run protocol tests**

```bash
cargo test -p transcoderr --lib worker::protocol 2>&1 | tail -10
```

Expected: 9 passed (8 existing from Pieces 1+3+4+5, plus 1 new).

- [ ] **Step 6: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/protocol.rs
git commit -m "feat(worker): step_cancel protocol variant"
```

---

## Task 2: Worker-side `step_cancellations` registry plumbing

Add a per-connection `Arc<RwLock<HashMap<String, CancellationToken>>>` in `worker/connection.rs::connect_once`, and thread it into the spawned `handle_step_dispatch` task. Don't yet wire the receive-loop arm or the executor's register/unregister — those land in Tasks 3 and 4. This task is pure plumbing.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs`
- Modify: `crates/transcoderr/src/worker/executor.rs` (signature change only — new parameter)

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the registry to `connect_once`**

Read the current `connect_once` body to find where the sync_slot + sync_notify are set up (Piece 4 / Piece 5 leftovers, around lines 95-120):

```bash
sed -n '70,180p' crates/transcoderr/src/worker/connection.rs
```

After the sync_slot / sync_notify setup block, BEFORE the register frame is built/sent, add:

```rust
    // Per-connection cancel registry. Keyed by correlation_id (the
    // step_dispatch envelope.id). `handle_step_dispatch` registers a
    // fresh token at dispatch start; the receive loop's StepCancel
    // arm fires it. Lives for the connection's lifetime.
    let step_cancellations: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, tokio_util::sync::CancellationToken>,
        >,
    > = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
```

(`tokio_util::sync::CancellationToken` is already used by the coordinator side via `JobCancellations`; no new dep.)

- [ ] **Step 3: Pass the registry into `handle_step_dispatch`**

Find the existing place in `connect_once` where `Message::StepDispatch(dispatch)` triggers the spawned executor task (it's inside the receive loop's match arms, set up by Piece 3). The current code looks roughly like:

```rust
                Message::StepDispatch(dispatch) => {
                    let tx_for_dispatch = outbound_tx.clone();
                    let correlation_id = envelope.id.clone();
                    tokio::spawn(async move {
                        crate::worker::executor::handle_step_dispatch(
                            tx_for_dispatch,
                            correlation_id,
                            dispatch,
                        )
                        .await;
                    });
                }
```

Update to also clone + pass the registry:

```rust
                Message::StepDispatch(dispatch) => {
                    let tx_for_dispatch = outbound_tx.clone();
                    let correlation_id = envelope.id.clone();
                    let cancellations = step_cancellations.clone();
                    tokio::spawn(async move {
                        crate::worker::executor::handle_step_dispatch(
                            tx_for_dispatch,
                            correlation_id,
                            dispatch,
                            cancellations,
                        )
                        .await;
                    });
                }
```

- [ ] **Step 4: Update `handle_step_dispatch` signature in executor.rs**

In `crates/transcoderr/src/worker/executor.rs`, change the function signature only (Task 3 fills in the body that uses the new parameter):

Before (current):
```rust
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
) {
```

After:
```rust
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
    step_cancellations: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, tokio_util::sync::CancellationToken>,
        >,
    >,
) {
```

For now, **add a leading `_` to the parameter name** (`_step_cancellations`) so the unused-variable warning is suppressed. Task 3 removes the underscore when it actually uses the registry.

```rust
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
    _step_cancellations: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, tokio_util::sync::CancellationToken>,
        >,
    >,
) {
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean. The `_` prefix prevents an unused-variable warning.

- [ ] **Step 6: Run worker_connect + remote_dispatch tests (regression net)**

```bash
cargo test -p transcoderr --test worker_connect --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: 4 + 5 passed. The signature change is additive; existing tests don't construct `handle_step_dispatch` directly.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs \
        crates/transcoderr/src/worker/executor.rs
git commit -m "feat(worker): plumb per-connection step_cancellations into executor"
```

---

## Task 3: `handle_step_dispatch` registers + unregisters cancel tokens

Use the parameter introduced in Task 2. Register a fresh token at dispatch start, attach to `Context.cancel`, run the step, unregister on completion. The existing transcode + subprocess steps already read `ctx.cancel.cancelled()` to abort — no step impl changes needed.

**Files:**
- Modify: `crates/transcoderr/src/worker/executor.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Drop the `_` prefix and use the registry**

In `crates/transcoderr/src/worker/executor.rs`, the function is currently `_step_cancellations: ...` (Task 2). Change it to `step_cancellations: ...` (drop the underscore) and use it in the body:

After the existing `// 1. Parse the context.` block (around line 30) and BEFORE the existing `// 2. Resolve the step from the registry.` block, add:

```rust
    // NEW (Piece 6): register a fresh cancel token for this dispatch
    // and attach it to ctx.cancel. Existing transcode + subprocess
    // steps read ctx.cancel.cancelled() to abort their work; the
    // worker-side StepCancel envelope handler (in connection.rs)
    // fires this token by correlation_id.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    step_cancellations
        .write()
        .await
        .insert(correlation_id.clone(), cancel_token.clone());
    ctx.cancel = Some(cancel_token);
```

Then at the END of the function — AFTER the `match result { Ok(()) => ... | Err(e) => ... }` block — add the unregister:

```rust
    // NEW (Piece 6): unregister the cancel token. Done after
    // send_complete so a late StepCancel arriving between step.execute
    // returning and unregister still fires the token (a no-op since
    // the step is already done). Idempotent cleanup.
    step_cancellations.write().await.remove(&correlation_id);
```

The full structure is:

```rust
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
    step_cancellations: std::sync::Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, tokio_util::sync::CancellationToken>,
        >,
    >,
) {
    let StepDispatch { job_id, step_id, use_, with, ctx_snapshot } = dispatch;

    // 1. Parse the context.
    let mut ctx = match Context::from_snapshot(&ctx_snapshot) {
        // existing match arms unchanged
    };

    // NEW: register cancel token + attach to ctx.cancel.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    step_cancellations
        .write()
        .await
        .insert(correlation_id.clone(), cancel_token.clone());
    ctx.cancel = Some(cancel_token);

    // 2. Resolve the step from the registry.
    // … existing body unchanged …

    // 5. Execute. Errors become `step_complete{failed}`.
    let result = step.execute(&with_map, &mut ctx, &mut cb).await;

    match result {
        // existing match arms unchanged
    }

    // NEW: unregister cancel token.
    step_cancellations.write().await.remove(&correlation_id);
}
```

**IMPORTANT — early returns**: the existing function has multiple early `return;` statements (e.g. on snapshot parse failure, registry resolve failure, with-map type mismatch). For those paths, the cancel token was NEVER inserted (because we only insert AFTER the snapshot parse), so there's nothing to clean up. Verify by reading the early-return sites: each returns BEFORE the `let cancel_token = ...` line.

If you choose to register BEFORE the snapshot parse (a defensive choice), you'd need to add unregister to every early-return path. **Don't do that** — keep the register call after the snapshot parse so there's only one cleanup site at the end.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean. The `_step_cancellations` warning from Task 2 goes away.

- [ ] **Step 4: Run worker_connect + remote_dispatch tests**

```bash
cargo test -p transcoderr --test worker_connect --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: 4 + 5 passed. The cancel token attachment is invisible to existing tests (they don't trigger cancel mid-step).

- [ ] **Step 5: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/executor.rs
git commit -m "feat(worker): handle_step_dispatch attaches cancel token to ctx.cancel"
```

---

## Task 4: Worker receive-loop `Message::StepCancel` arm

The receive loop in `connect_once` already handles `StepDispatch`, `PluginSync`, etc. Add a `StepCancel` arm that fires the matching token by correlation_id.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Find the receive loop's match block**

```bash
grep -nE "Message::(StepDispatch|PluginSync|Heartbeat)" crates/transcoderr/src/worker/connection.rs | head
```

You'll see (Piece 3 / 4 / 5 wired these). Inside the receive loop's `match envelope.message` block, add a new arm alongside the existing `Message::StepDispatch`, `Message::PluginSync`, `Message::RegisterAck`, etc.:

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
                        // Race: cancel arrived after step_complete already
                        // fired (handle_step_dispatch removed the entry).
                        // No-op; debug log only.
                        tracing::debug!(
                            correlation_id = %envelope.id,
                            "step cancel for unknown correlation; dropped"
                        );
                    }
                }
```

Place it as a sibling of the existing arms (inside the same `match` block). The existing `_ => { ... }` catch-all (if any) stays at the bottom.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 4: Run worker_connect + remote_dispatch + plugin_remote_dispatch (cross-piece sanity)**

```bash
cargo test -p transcoderr --test worker_connect --test remote_dispatch --test plugin_remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: 4 + 5 + 5 passed.

- [ ] **Step 5: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs
git commit -m "feat(worker): receive-loop StepCancel arm fires registered token"
```

---

## Task 5: `RemoteRunner::run` cancel-watching `tokio::select!` arm

The critical-path change. Adds a third arm to the existing inbound-frame `tokio::select!` loop in `RemoteRunner::run`. On cancel, sends `StepCancel` to the worker and bails immediately. **Pause for user confirmation after this task** — every existing remote dispatch flows through this code.

**Files:**
- Modify: `crates/transcoderr/src/dispatch/remote.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read the current loop body**

```bash
sed -n '40,120p' crates/transcoderr/src/dispatch/remote.rs
```

Note the current shape:

```rust
        // 3. Pump inbound frames until completion or timeout.
        loop {
            let frame = match tokio::time::timeout(STEP_FRAME_TIMEOUT, rx.recv()).await {
                Ok(Some(f)) => f,
                Ok(None) => anyhow::bail!("worker inbox channel closed"),
                Err(_) => anyhow::bail!("worker step timed out"),
            };
            match frame {
                InboundStepEvent::Progress(p) => { /* … */ }
                InboundStepEvent::Complete(c) => { /* … */ }
            }
        }
```

- [ ] **Step 3: Replace the `loop` body with a `tokio::select!` that also watches cancel**

Update the import block at the top of the file to add `StepCancelMsg`:

Before:
```rust
use crate::worker::protocol::{Envelope, Message, StepDispatch};
```

After:
```rust
use crate::worker::protocol::{Envelope, Message, StepCancelMsg, StepDispatch};
```

Then replace the entire `// 3. Pump inbound frames…` `loop { … }` block with:

```rust
        // 3. Pump inbound frames until completion, timeout, or cancel.
        let cancel = ctx.cancel.clone(); // Option<CancellationToken>
        loop {
            let frame = tokio::select! {
                f = tokio::time::timeout(STEP_FRAME_TIMEOUT, rx.recv()) => match f {
                    Ok(Some(f)) => f,
                    Ok(None) => anyhow::bail!("worker inbox channel closed"),
                    Err(_) => anyhow::bail!("worker step timed out"),
                },
                _ = async {
                    // If ctx.cancel is None (test fixtures, edge cases),
                    // this branch never resolves — the loop behaves
                    // exactly as today.
                    match &cancel {
                        Some(c) => c.cancelled().await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    // Operator cancelled the job. Send StepCancel to the
                    // worker (fire-and-forget — Piece 6 spec Q1-A) and
                    // bail. Engine records the run as cancelled via the
                    // existing cancel-token-aware error path.
                    let cancel_env = Envelope {
                        id: correlation_id.clone(),
                        message: Message::StepCancel(StepCancelMsg {
                            job_id,
                            step_id: step_id.into(),
                        }),
                    };
                    let _ = state
                        .connections
                        .send_to_worker(worker_id, cancel_env)
                        .await;
                    anyhow::bail!("step cancelled by operator");
                }
            };

            match frame {
                InboundStepEvent::Progress(p) => {
                    let progress = match p.kind.as_str() {
                        "progress" => {
                            let pct = p.payload.get("pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            StepProgress::Pct(pct)
                        }
                        "log" => {
                            let msg = p
                                .payload
                                .get("msg")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            StepProgress::Log(msg)
                        }
                        other => StepProgress::Marker {
                            kind: other.to_string(),
                            payload: p.payload,
                        },
                    };
                    on_progress(progress);
                }
                InboundStepEvent::Complete(c) => {
                    if c.status == "ok" {
                        if let Some(snap) = c.ctx_snapshot {
                            // Preserve cancel-token across the snapshot
                            // restore. Context::cancel is #[serde(skip)],
                            // so deserialising a snapshot loses it. Without
                            // this, any local follow-on step in the same
                            // flow would lose cancellation propagation.
                            let cancel = ctx.cancel.clone();
                            *ctx = Context::from_snapshot(&snap)?;
                            ctx.cancel = cancel;
                        }
                        return Ok(());
                    }
                    anyhow::bail!(
                        "remote step failed: {}",
                        c.error.unwrap_or_else(|| "unknown error".into())
                    );
                }
            }
        }
```

The body inside `match frame { ... }` is **identical** to the existing code — only the outer frame-grabbing path changed from a plain `match tokio::time::timeout(...)` to a `tokio::select!` with a cancel arm.

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean. Common compile errors:
- Missing `StepCancelMsg` import → fix the use line at the top.
- `cancel.clone()` complaints — `ctx.cancel` is `Option<CancellationToken>`, which is Clone.
- Lifetime / move issues in the select arm — `cancel` is captured by reference inside the async block; should compile fine since the block holds a `&Option<...>` clone.

- [ ] **Step 5: Critical-path tests must stay green**

```bash
cargo test -p transcoderr --test concurrent_claim --test crash_recovery --test flow_engine 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: every line `test result: ok.`. NO FAILED. These tests don't use cancel, but they exercise the engine + dispatch path that the modified RemoteRunner participates in.

- [ ] **Step 6: Run remote_dispatch + plugin_remote_dispatch (these directly exercise RemoteRunner)**

```bash
cargo test -p transcoderr --test remote_dispatch --test plugin_remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: 5 + 5 passed. The select! restructure preserves all existing frame-handling semantics; existing tests should pass unchanged.

- [ ] **Step 7: Lib + Piece 1/2/3/4/5 integration tests**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test worker_connect --test local_worker --test plugin_push --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/dispatch/remote.rs
git commit -m "feat(dispatch): RemoteRunner watches ctx.cancel + sends StepCancel on fire"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 6: Integration tests `tests/cancel_remote.rs`

End-to-end verification of the cancel propagation path. 3 scenarios. Reuses the fake-worker harness pattern from Piece 3's `tests/remote_dispatch.rs`.

**Files:**
- Create: `crates/transcoderr/tests/cancel_remote.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read the existing fake-worker harness**

```bash
head -100 crates/transcoderr/tests/remote_dispatch.rs
```

Note the canonical helpers: `mint_token`, `ws_connect`, `send_env`, `recv_env`, `send_register_and_get_ack`, `submit_job_with_step`, `wait_for_step_dispatch`. Reuse these patterns.

- [ ] **Step 3: Verify how to trigger cancel from a test**

```bash
grep -rnE "JobCancellations|api_jobs.*cancel|fn cancel" crates/transcoderr/src/api/jobs.rs | head -10
```

There's an existing `POST /api/jobs/:id/cancel` endpoint. Tests can hit it via reqwest, OR directly call `app.state.cancellations.cancel(job_id)`. The latter is simpler and more direct. The plan uses the direct path — read `tests/common/mod.rs` to confirm `app.state.cancellations` is exposed (the `state` field was added in Piece 4).

- [ ] **Step 4: Create the test file**

```rust
//! Integration tests for Piece 6's cancel propagation:
//!  1. cancel_propagates_to_remote_worker
//!  2. cancel_unblocks_engine_within_a_second
//!  3. cancel_after_step_complete_is_silent

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register, StepComplete,
};

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send().await.unwrap()
        .json().await.unwrap();
    (
        resp["id"].as_i64().unwrap(),
        resp["secret_token"].as_str().unwrap().to_string(),
    )
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let mut req = format!("{base_ws}/api/worker/connect")
        .as_str()
        .into_client_request()
        .unwrap();
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

async fn send_env(ws: &mut Ws, env: &Envelope) {
    let s = serde_json::to_string(env).unwrap();
    ws.send(WsMessage::Text(s)).await.unwrap();
}

async fn recv_env(ws: &mut Ws) -> Envelope {
    let raw = ws.next().await.unwrap().unwrap();
    match raw {
        WsMessage::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn send_register_and_get_ack(
    ws: &mut Ws,
    name: &str,
    available_steps: Vec<String>,
) -> Envelope {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps,
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    recv_env(ws).await
}

/// Insert a flow + a job that points at a specific step kind. Mirrors
/// `tests/remote_dispatch.rs::submit_job_with_step`.
async fn submit_job_with_step(
    app: &common::TestApp,
    use_: &str,
    run_on: Option<&str>,
) -> (i64, i64) {
    let run_on_clause = match run_on {
        Some(r) => format!("    run_on: {r}\n"),
        None => "".into(),
    };
    let yaml = format!(
        "name: t\ntriggers: [{{ webhook: x }}]\nsteps:\n  - use: {use_}\n{run_on_clause}"
    );
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let parsed_json = serde_json::to_string(&value).unwrap();

    sqlx::query("INSERT INTO flows (name, yaml_source, parsed_json, enabled, created_at) VALUES (?, ?, ?, 1, strftime('%s','now'))")
        .bind("t").bind(&yaml).bind(&parsed_json)
        .execute(&app.pool).await.unwrap();
    let flow_id: i64 = sqlx::query_scalar("SELECT id FROM flows ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    sqlx::query("INSERT INTO jobs (flow_id, file_path, status, created_at) VALUES (?, '/tmp/x.mkv', 'pending', strftime('%s','now'))")
        .bind(flow_id).execute(&app.pool).await.unwrap();
    let job_id: i64 = sqlx::query_scalar("SELECT id FROM jobs ORDER BY id DESC LIMIT 1")
        .fetch_one(&app.pool).await.unwrap();
    (flow_id, job_id)
}

/// Wait for the next StepDispatch envelope; return None on timeout.
async fn wait_for_step_dispatch(
    ws: &mut Ws,
    deadline: Duration,
) -> Option<Envelope> {
    let res = tokio::time::timeout(deadline, async {
        loop {
            let env = recv_env(ws).await;
            if matches!(env.message, Message::StepDispatch(_)) {
                return env;
            }
        }
    })
    .await;
    res.ok()
}

/// Wait for the next StepCancel envelope; return None on timeout.
async fn wait_for_step_cancel(
    ws: &mut Ws,
    deadline: Duration,
) -> Option<Envelope> {
    let res = tokio::time::timeout(deadline, async {
        loop {
            let env = recv_env(ws).await;
            if matches!(env.message, Message::StepCancel(_)) {
                return env;
            }
        }
    })
    .await;
    res.ok()
}

/// Poll the jobs table for `job_id`'s status, blocking up to
/// `deadline`. Returns the final status.
async fn wait_for_job_status(
    pool: &sqlx::SqlitePool,
    job_id: i64,
    target: &str,
    deadline: Duration,
) -> Option<String> {
    let start = std::time::Instant::now();
    loop {
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM jobs WHERE id = ?",
        )
        .bind(job_id)
        .fetch_optional(pool)
        .await
        .unwrap();
        if let Some(s) = &status {
            if s == target {
                return Some(s.clone());
            }
        }
        if start.elapsed() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn cancel_propagates_to_remote_worker() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_cancel").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker advertises transcode (a known built-in with Executor::Any).
    send_register_and_get_ack(&mut ws, "fake_cancel", vec!["transcode".into()]).await;

    // Submit a job. Wait for the dispatch to arrive at the fake worker.
    let (_flow_id, job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5))
        .await
        .expect("worker should receive step_dispatch within 5s");

    // Trigger cancel via the cancellations registry directly.
    // (Equivalent to the operator clicking Cancel in the UI.)
    assert!(
        app.state.cancellations.cancel(job_id),
        "cancel should find the registered token"
    );

    // Worker should receive a StepCancel envelope within 1s.
    let cancel_env = wait_for_step_cancel(&mut ws, Duration::from_secs(1))
        .await
        .expect("worker should receive step_cancel within 1s");

    // Verify the StepCancel correlates with the original dispatch.
    assert_eq!(
        cancel_env.id, dispatch.id,
        "step_cancel correlation_id must match step_dispatch"
    );
    match cancel_env.message {
        Message::StepCancel(p) => {
            assert_eq!(p.job_id, job_id);
        }
        other => panic!("expected StepCancel, got {other:?}"),
    }

    // Engine records the run as cancelled within 2s.
    let final_status = wait_for_job_status(
        &app.pool,
        job_id,
        "cancelled",
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(
        final_status.as_deref(),
        Some("cancelled"),
        "job should reach cancelled status within 2s (got {final_status:?})"
    );
}

#[tokio::test]
async fn cancel_unblocks_engine_within_a_second() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_fast").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_get_ack(&mut ws, "fake_fast", vec!["transcode".into()]).await;

    let (_flow_id, job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;
    let _dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await.unwrap();

    // Measure wall-clock from cancel call to job reaching cancelled.
    let start = std::time::Instant::now();
    app.state.cancellations.cancel(job_id);
    let final_status = wait_for_job_status(
        &app.pool,
        job_id,
        "cancelled",
        Duration::from_secs(5),
    )
    .await;
    let elapsed = start.elapsed();

    assert_eq!(
        final_status.as_deref(),
        Some("cancelled"),
        "job should reach cancelled status (got {final_status:?})"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel→cancelled should be under 2s, got {elapsed:?} (well below the 30s frame timeout)"
    );
}

#[tokio::test]
async fn cancel_after_step_complete_is_silent() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_done").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_get_ack(&mut ws, "fake_done", vec!["transcode".into()]).await;

    let (_flow_id, job_id) = submit_job_with_step(&app, "transcode", Some("any")).await;
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await.unwrap();
    let correlation_id = dispatch.id.clone();
    let step_id = match dispatch.message {
        Message::StepDispatch(d) => d.step_id,
        _ => unreachable!(),
    };

    // Reply with step_complete{ok} immediately. The engine completes
    // the run and tears down the inbox.
    let complete = Envelope {
        id: correlation_id,
        message: Message::StepComplete(StepComplete {
            job_id,
            step_id,
            status: "ok".into(),
            error: None,
            ctx_snapshot: Some("{}".into()),
        }),
    };
    send_env(&mut ws, &complete).await;

    // Wait for the run to reach completed.
    let _ = wait_for_job_status(&app.pool, job_id, "completed", Duration::from_secs(5)).await;

    // NOW cancel — should be a no-op (token was unregistered when
    // RemoteRunner returned Ok). No StepCancel envelope should arrive
    // at the fake worker within 1s.
    let _ = app.state.cancellations.cancel(job_id);
    let cancel_env = wait_for_step_cancel(&mut ws, Duration::from_secs(1)).await;
    assert!(
        cancel_env.is_none(),
        "no step_cancel envelope should arrive after step_complete"
    );
}
```

Notes:
- `app.state.cancellations` is exposed because `TestApp.state: AppState` was added in Piece 4 Task 12.
- The cancel API endpoint exists at `POST /api/jobs/:id/cancel`; tests could use that path instead via reqwest, but `app.state.cancellations.cancel(job_id)` is simpler and more direct.
- The `cancel_propagates_to_remote_worker` test asserts both halves: (a) the worker receives StepCancel within 1s, AND (b) the engine records cancelled within 2s. If only (a) succeeds and (b) fails, the engine isn't propagating the cancel — investigate `Engine::run_nodes`'s cancel-token handling.
- The `cancel_after_step_complete_is_silent` test verifies the unregister-on-complete path. If a StepCancel envelope arrives, the worker's cancel registry still had the entry — investigate the unregister timing in Task 3.

- [ ] **Step 5: Run the new tests**

```bash
cargo test -p transcoderr --test cancel_remote 2>&1 | tail -15
```

Expected: 3 passed.

If a test hangs, the most likely cause:
- Test 1 / 2: cancel signal isn't reaching `RemoteRunner`. Verify Task 5's tokio::select arm fires by adding a `tracing::info!` log in the cancel branch and re-running with `RUST_LOG=debug`.
- Test 3: a StepCancel IS arriving at the fake worker after step_complete. Investigate the unregister timing — Task 3 says "after send_complete" but we may need to verify ordering. If RemoteRunner returns Ok, then the engine moves on (no cancel triggered), so no StepCancel should be sent. The `app.state.cancellations.cancel(job_id)` returns false (no token registered), so no StepCancel is constructed.

- [ ] **Step 6: Run the full integration suite for confidence**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-6" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/cancel_remote.rs
git commit -m "test(cancel_remote): 3-scenario cancel propagation integration suite"
```

---

## Self-Review Notes

This plan covers every section of the spec:

- **`StepCancel` wire envelope** → Task 1.
- **Worker-side per-connection `step_cancellations` registry** → Task 2 (plumbing).
- **`handle_step_dispatch` registers + unregisters cancel tokens; attaches to `Context.cancel`** → Task 3.
- **Worker receive-loop `Message::StepCancel` arm** → Task 4.
- **Coordinator-side `RemoteRunner` cancel-watching select arm** → Task 5.
- **Fire-and-forget cancel semantics (Q1-A)** → Task 5 (`anyhow::bail!` immediately after sending StepCancel).
- **Idle-sweep stays observation-only (Q2-A)** → no task; spec confirms no work needed.
- **Reconnect-with-in-flight-work deferred (Q3-A)** → no task; spec confirms deferred.
- **3-scenario integration suite** → Task 6.

Cross-task type/signature consistency:

- `StepCancel(StepCancelMsg { job_id: i64, step_id: String })` (Task 1) — referenced in coordinator's RemoteRunner (Task 5) when sending, and in worker's receive loop (Task 4) when handling. Field names match.
- `step_cancellations: Arc<RwLock<HashMap<String, CancellationToken>>>` (Task 2) — declared in `connect_once` (Task 2), passed to `handle_step_dispatch` (Task 2 signature, Task 3 body), read by receive-loop arm (Task 4). Type signature stable.
- `correlation_id: String` is the lookup key everywhere — matches `envelope.id` from Piece 1's protocol.
- `tokio_util::sync::CancellationToken` is the token type — already used by `JobCancellations` (existing `cancellation.rs:13`); no new dep.
- Existing `Context.cancel: Option<CancellationToken>` field (`#[serde(skip)]`) — Task 3 sets it to `Some(token)` after `Context::from_snapshot`; Task 5's `RemoteRunner` reads it via `ctx.cancel.clone()`. Same semantics on both sides.
- `app.state.cancellations.cancel(job_id)` (Task 6 test) — returns `bool` (true if a token was found and fired). Existing API from `cancellation.rs:33`.

No placeholders. Every step has executable code or exact commands. All file paths absolute. Bite-sized step granularity (each step is a 2-5 minute action). Frequent commits — 6 total commits, one per task. **Smallest piece in the roadmap; closes out issue #79.**
