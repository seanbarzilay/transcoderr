//! Worker-side step executor. The connection loop calls
//! `handle_step_dispatch` on each `step_dispatch` envelope; this
//! module runs the step using the same registry the local pool
//! uses and replies with `step_complete`.
//!
//! No SqlitePool / AppState is needed here — `registry::resolve`
//! reads the global `OnceCell` registry that the worker daemon
//! initialises at boot, and `Step::execute` carries any state it
//! needs inside `&self`.

use crate::flow::Context;
use crate::steps::{registry, StepProgress};
use crate::worker::protocol::{
    Envelope, Message, StepComplete, StepDispatch, StepProgressMsg,
};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

/// Run one dispatched step end-to-end and send `step_complete`. The
/// `tx` channel is the same outbound mpsc the connection loop uses
/// for heartbeats; everything we send goes through it.
pub async fn handle_step_dispatch(
    tx: mpsc::Sender<Envelope>,
    correlation_id: String,
    dispatch: StepDispatch,
) {
    let StepDispatch { job_id, step_id, use_, with, ctx_snapshot } = dispatch;

    // 1. Parse the context.
    let mut ctx = match Context::from_snapshot(&ctx_snapshot) {
        Ok(c) => c,
        Err(e) => {
            send_complete(
                &tx,
                &correlation_id,
                job_id,
                &step_id,
                "failed",
                Some(format!("ctx parse: {e}")),
                None,
            )
            .await;
            return;
        }
    };

    // 2. Resolve the step from the registry.
    let step = match registry::resolve(&use_).await {
        Some(s) => s,
        None => {
            send_complete(
                &tx,
                &correlation_id,
                job_id,
                &step_id,
                "failed",
                Some(format!("unknown step `{use_}`")),
                None,
            )
            .await;
            return;
        }
    };

    // 3. Translate the YAML `with` JSON Value into the BTreeMap
    //    shape the Step trait wants.
    let with_map: BTreeMap<String, serde_json::Value> = match with {
        serde_json::Value::Object(m) => m.into_iter().collect(),
        serde_json::Value::Null => BTreeMap::new(),
        other => {
            send_complete(
                &tx,
                &correlation_id,
                job_id,
                &step_id,
                "failed",
                Some(format!("`with` is not an object: {other:?}")),
                None,
            )
            .await;
            return;
        }
    };

    // 4. Build the on_progress callback that ships StepProgress
    //    events back to the coordinator as `step_progress` envelopes.
    let tx_for_cb = tx.clone();
    let correlation_for_cb = correlation_id.clone();
    let step_id_for_cb = step_id.clone();
    let mut cb = move |ev: StepProgress| {
        let tx = tx_for_cb.clone();
        let correlation = correlation_for_cb.clone();
        let step_id = step_id_for_cb.clone();
        tokio::spawn(async move {
            let (kind, payload) = match ev {
                StepProgress::Pct(p) => (
                    "progress".to_string(),
                    serde_json::json!({ "pct": p }),
                ),
                StepProgress::Log(l) => (
                    "log".to_string(),
                    serde_json::json!({ "msg": l }),
                ),
                StepProgress::Marker { kind, payload } => (kind, payload),
            };
            let env = Envelope {
                id: correlation,
                message: Message::StepProgress(StepProgressMsg {
                    job_id,
                    step_id,
                    kind,
                    payload,
                }),
            };
            let _ = tx.send(env).await;
        });
    };

    // 5. Execute. Errors become `step_complete{failed}`.
    let result = step.execute(&with_map, &mut ctx, &mut cb).await;

    match result {
        Ok(()) => {
            let snap = Some(ctx.to_snapshot());
            send_complete(
                &tx,
                &correlation_id,
                job_id,
                &step_id,
                "ok",
                None,
                snap,
            )
            .await;
        }
        Err(e) => {
            send_complete(
                &tx,
                &correlation_id,
                job_id,
                &step_id,
                "failed",
                Some(e.to_string()),
                None,
            )
            .await;
        }
    }
}

async fn send_complete(
    tx: &mpsc::Sender<Envelope>,
    correlation_id: &str,
    job_id: i64,
    step_id: &str,
    status: &str,
    error: Option<String>,
    ctx_snapshot: Option<String>,
) {
    let env = Envelope {
        id: correlation_id.into(),
        message: Message::StepComplete(StepComplete {
            job_id,
            step_id: step_id.into(),
            status: status.into(),
            error,
            ctx_snapshot,
        }),
    };
    let _ = tx.send(env).await;
}
