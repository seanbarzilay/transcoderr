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

/// Inter-frame timeout: how long we wait between progress frames once
/// the worker has emitted *something* for this step. Long enough to
/// ride out network blips, short enough to fail a stuck flow promptly.
const STEP_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// First-frame timeout: how long we wait for the worker to emit its
/// first frame (any kind) for this step. Generous because heavy
/// ffmpeg invocations (UHD HEVC, software encoding, cold storage)
/// can legitimately spend several minutes on file-open + index +
/// codec init before printing their first `progress=` line, and the
/// worker only relays frames once ffmpeg actually starts producing
/// output.
const STEP_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(300);

pub struct RemoteRunner;

pub struct RemoteStep<'a> {
    pub worker_id: i64,
    pub job_id: i64,
    pub step_id: &'a str,
    pub use_: &'a str,
    pub with: &'a BTreeMap<String, serde_json::Value>,
}

impl RemoteRunner {
    /// Run a single step on a remote worker. Blocks until the worker
    /// reports `step_complete` (success or failure), the frame timeout
    /// fires, or `ctx.cancel` is signalled by the operator (in which
    /// case we send `StepCancel` to the worker fire-and-forget and
    /// bail with `"step cancelled by operator"`).
    ///
    /// On Ok: `ctx` has been replaced with the worker's returned
    /// context snapshot (with paths reverse-mapped back to coordinator
    /// space when the worker has path mappings configured).
    pub async fn run(
        state: &AppState,
        step: RemoteStep<'_>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let correlation_id = format!("dsp-{}", uuid::Uuid::new_v4());

        // 0. Load (or lazily fill) the per-worker path mappings cache.
        //    Snapshot once for the duration of this step so a mid-flight
        //    edit by the operator can't desync the round-trip.
        let mappings = load_or_fill_mappings(state, step.worker_id).await;

        // 1. Register an inbox for inbound frames keyed by correlation_id.
        let (mut rx, _inbox_guard) = state
            .connections
            .register_inbox(correlation_id.clone())
            .await;

        // 2. Build the context snapshot, rewriting paths on the way out.
        let ctx_snapshot = if mappings.is_empty() {
            ctx.to_snapshot()
        } else {
            let mut value: serde_json::Value = serde_json::from_str(&ctx.to_snapshot())?;
            mappings.apply(&mut value, crate::path_mapping::Direction::CoordToWorker);
            serde_json::to_string(&value)?
        };

        let with_json: serde_json::Value = serde_json::to_value(step.with)?;
        let dispatch_env = Envelope {
            id: correlation_id.clone(),
            message: Message::StepDispatch(StepDispatch {
                job_id: step.job_id,
                step_id: step.step_id.into(),
                use_: step.use_.into(),
                with: with_json,
                ctx_snapshot,
            }),
        };
        state
            .connections
            .send_to_worker(step.worker_id, dispatch_env)
            .await
            .map_err(|e| anyhow::anyhow!("dispatch send failed: {e}"))?;

        // 3. Pump inbound frames until completion, timeout, or cancel.
        //    Use the lenient `STEP_FIRST_FRAME_TIMEOUT` until the
        //    worker has emitted any frame; tighten to
        //    `STEP_FRAME_TIMEOUT` for subsequent frames so a stuck
        //    mid-stream worker still gets caught quickly.
        let cancel = ctx.cancel.clone();
        let mut first_frame_seen = false;
        loop {
            let timeout = if first_frame_seen {
                STEP_FRAME_TIMEOUT
            } else {
                STEP_FIRST_FRAME_TIMEOUT
            };
            let frame = tokio::select! {
                f = tokio::time::timeout(timeout, rx.recv()) => match f {
                    Ok(Some(f)) => f,
                    Ok(None) => anyhow::bail!("worker inbox channel closed"),
                    Err(_) => anyhow::bail!("worker step timed out"),
                },
                _ = async {
                    // Explicit disconnect-watch. Without this, a worker
                    // that drops its WS before sending any frame would
                    // sit silently until STEP_FIRST_FRAME_TIMEOUT (5min)
                    // — the inbox sender stays alive in the
                    // Connections registry until our InboxGuard drops.
                    // Polling is_connected every 2s catches the
                    // disconnect within ~2s of SenderGuard cleanup.
                    loop {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if !state.connections.is_connected(step.worker_id).await {
                            return;
                        }
                    }
                } => {
                    anyhow::bail!("worker disconnected mid-step");
                }
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
                        job_id = step.job_id,
                        step_id = step.step_id,
                        worker_id = step.worker_id,
                        correlation_id = %correlation_id,
                        "cancelling in-flight remote step; sending StepCancel to worker"
                    );
                    let cancel_env = Envelope {
                        id: correlation_id.clone(),
                        message: Message::StepCancel(StepCancelMsg {
                            job_id: step.job_id,
                            step_id: step.step_id.into(),
                        }),
                    };
                    let _ = state
                        .connections
                        .send_to_worker(step.worker_id, cancel_env)
                        .await;
                    anyhow::bail!("step cancelled by operator");
                }
            };

            // Reaching here means we successfully extracted a frame
            // (the only fall-through arm of the select; cancel and
            // timeout/recv-failed branches all bail). Tighten the
            // timeout for subsequent frames.
            first_frame_seen = true;

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
                            // Reverse-rewrite paths on the way back so
                            // the next step on the coordinator sees
                            // coordinator-space paths.
                            let restored = if mappings.is_empty() {
                                snap
                            } else {
                                let mut value: serde_json::Value = serde_json::from_str(&snap)?;
                                mappings.apply(
                                    &mut value,
                                    crate::path_mapping::Direction::WorkerToCoord,
                                );
                                serde_json::to_string(&value)?
                            };
                            let cancel = ctx.cancel.clone();
                            *ctx = Context::from_snapshot(&restored)?;
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

/// Look up the cached `PathMappings` for `worker_id`. If there's no
/// entry, load from `workers.path_mappings_json`, populate the cache,
/// and return the loaded value. Errors are non-fatal: a parse failure
/// is logged and the dispatch falls back to identity translation.
async fn load_or_fill_mappings(
    state: &AppState,
    worker_id: i64,
) -> crate::path_mapping::PathMappings {
    if let Some(cached) = state.connections.path_mappings_for(worker_id).await {
        return cached;
    }
    let row = match crate::db::workers::get_by_id(&state.pool, worker_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return crate::path_mapping::PathMappings::default(),
        Err(e) => {
            tracing::warn!(worker_id, error = ?e, "load_or_fill_mappings: db read failed; falling back to identity");
            return crate::path_mapping::PathMappings::default();
        }
    };
    let mappings = match row.path_mappings_json.as_deref() {
        Some(s) => match crate::path_mapping::PathMappings::from_json(s) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    worker_id,
                    error = ?e,
                    "path_mappings_json failed to parse; falling back to identity"
                );
                crate::path_mapping::PathMappings::default()
            }
        },
        None => crate::path_mapping::PathMappings::default(),
    };
    state
        .connections
        .set_path_mappings(worker_id, mappings.clone())
        .await;
    mappings
}
