//! `RemoteRunner` — opens `step_dispatch` over the worker's WS,
//! awaits `step_complete`, maps `step_progress` to the engine's
//! on_progress callback. Called from `flow::engine::run_nodes` when
//! `dispatch::route` returns `Route::Remote(worker_id)`.

use crate::flow::Context;
use crate::http::AppState;
use crate::steps::StepProgress;
use crate::worker::connections::InboundStepEvent;
use crate::worker::protocol::{Envelope, Message, StepCancelMsg, StepDispatch};
use std::collections::BTreeMap;
use std::time::Duration;

/// Time we wait for any inbound frame from the worker before deciding
/// the dispatch is dead. Matches Piece 1's connection register
/// timeout semantics — long enough to ride out network blips, short
/// enough to fail a stuck flow promptly.
const STEP_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

pub struct RemoteRunner;

impl RemoteRunner {
    /// Run a single step on a remote worker. Blocks until the worker
    /// reports `step_complete` (success or failure), the frame timeout
    /// fires, or `ctx.cancel` is signalled by the operator (in which
    /// case we send `StepCancel` to the worker fire-and-forget and
    /// bail with `"step cancelled by operator"`).
    ///
    /// On Ok: `ctx` has been replaced with the worker's returned
    /// context snapshot.
    pub async fn run(
        state: &AppState,
        worker_id: i64,
        job_id: i64,
        step_id: &str,
        use_: &str,
        with: &BTreeMap<String, serde_json::Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let correlation_id = format!("dsp-{}", uuid::Uuid::new_v4());

        // 1. Register an inbox for inbound frames keyed by correlation_id.
        let (mut rx, _inbox_guard) = state
            .connections
            .register_inbox(correlation_id.clone())
            .await;

        // 2. Build and send the dispatch envelope.
        let with_json: serde_json::Value = serde_json::to_value(with)?;
        let dispatch_env = Envelope {
            id: correlation_id.clone(),
            message: Message::StepDispatch(StepDispatch {
                job_id,
                step_id: step_id.into(),
                use_: use_.into(),
                with: with_json,
                ctx_snapshot: ctx.to_snapshot(),
            }),
        };
        state
            .connections
            .send_to_worker(worker_id, dispatch_env)
            .await
            .map_err(|e| anyhow::anyhow!("dispatch send failed: {e}"))?;

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
                    tracing::info!(
                        job_id,
                        step_id,
                        worker_id,
                        correlation_id = %correlation_id,
                        "cancelling in-flight remote step; sending StepCancel to worker"
                    );
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
    }
}
