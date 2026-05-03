//! Integration tests for Piece 3's per-step dispatch + remote
//! execution. Spins up the in-process router, connects a scriptable
//! fake worker, exercises:
//!  1. step dispatched + completes
//!  2. progress events flow back into run_events
//!  3. mid-step disconnect fails the run within 30s
//!  4. coordinator-only steps run locally even with a worker present
//!  5. no eligible workers -> fall back to local

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::db;
use transcoderr::flow::parse_flow;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register,
    StepComplete, StepProgressMsg,
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
        resp["id"].as_i64().expect("id"),
        resp["secret_token"].as_str().expect("secret_token").to_string(),
    )
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let url = format!("{base_ws}/api/worker/connect");
    let mut req = url.as_str().into_client_request().unwrap();
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

async fn fake_worker_register(ws: &mut Ws, name: &str, available_steps: Vec<String>) {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({"encoders": []}),
            available_steps,
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    // Drain the register_ack.
    let _ack = recv_env(ws).await;
}

/// Insert a flow + a pending job using the canonical helpers (mirrors
/// `tests/local_worker.rs::submit_simple_flow_job`). The flow has a
/// single `use:` step, optionally tagged with `run_on:`.
///
/// Returns (flow_id, job_id).
async fn submit_job_with_step(
    app: &common::TestApp,
    flow_name: &str,
    use_: &str,
    run_on: Option<&str>,
) -> (i64, i64) {
    let run_on_line = match run_on {
        Some(r) => format!("    run_on: {r}\n"),
        None => String::new(),
    };
    let yaml = format!(
        "name: {flow_name}\ntriggers: [{{ webhook: x }}]\nsteps:\n  - use: {use_}\n{run_on_line}"
    );
    let flow = parse_flow(&yaml).unwrap();
    let flow_id = db::flows::insert(&app.pool, flow_name, &yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&app.pool, flow_id, 1, "webhook", "/tmp/x.mkv", "{}")
        .await
        .unwrap();
    (flow_id, job_id)
}

async fn job_status(pool: &sqlx::SqlitePool, id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Drain envelopes until a `step_dispatch` arrives or the deadline
/// fires. Heartbeats and other unrelated frames are skipped.
async fn wait_for_step_dispatch(ws: &mut Ws, deadline: Duration) -> Option<Envelope> {
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

#[tokio::test]
async fn step_dispatched_to_remote_worker_completes() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    fake_worker_register(&mut ws, "fake1", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "remote_completes", "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");
    let correlation_id = dispatch.id.clone();
    let (step_id, dispatched_ctx) = match dispatch.message {
        Message::StepDispatch(d) => (d.step_id, d.ctx_snapshot),
        _ => unreachable!(),
    };

    // Acknowledge with step_complete{ok}, echoing back the dispatched
    // context snapshot so the engine can deserialize it.
    let complete = Envelope {
        id: correlation_id,
        message: Message::StepComplete(StepComplete {
            job_id,
            step_id,
            status: "ok".into(),
            error: None,
            ctx_snapshot: Some(dispatched_ctx),
        }),
    };
    send_env(&mut ws, &complete).await;

    // Poll for terminal status.
    let mut completed = false;
    for _ in 0..50 {
        if job_status(&app.pool, job_id).await == "completed" {
            completed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(completed, "job should complete after step_complete{{ok}}");
}

#[tokio::test]
async fn progress_events_flow_back_to_run_events() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "fake_prog").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    fake_worker_register(&mut ws, "fake_prog", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "remote_progress", "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");
    let correlation_id = dispatch.id.clone();
    let (step_id, dispatched_ctx) = match dispatch.message {
        Message::StepDispatch(d) => (d.step_id, d.ctx_snapshot),
        _ => unreachable!(),
    };

    // Send two progress frames.
    for pct in [25.0_f64, 50.0_f64] {
        send_env(
            &mut ws,
            &Envelope {
                id: correlation_id.clone(),
                message: Message::StepProgress(StepProgressMsg {
                    job_id,
                    step_id: step_id.clone(),
                    kind: "progress".into(),
                    payload: json!({"pct": pct}),
                }),
            },
        )
        .await;
    }

    // Then complete (echo dispatched ctx so the engine can deserialize).
    send_env(
        &mut ws,
        &Envelope {
            id: correlation_id,
            message: Message::StepComplete(StepComplete {
                job_id,
                step_id,
                status: "ok".into(),
                error: None,
                ctx_snapshot: Some(dispatched_ctx),
            }),
        },
    )
    .await;

    // Wait for the engine to finish writing events.
    for _ in 0..50 {
        if job_status(&app.pool, job_id).await == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Tiny extra grace so the final event flush settles.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM run_events \
         WHERE job_id = ? AND kind = 'progress' AND worker_id = ?",
    )
    .bind(job_id)
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(
        count >= 2,
        "expected >=2 progress events stamped with worker_id (got {count})"
    );
}

#[tokio::test]
async fn disconnect_mid_step_fails_run() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_drop").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    fake_worker_register(&mut ws, "fake_drop", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "remote_drop", "transcode", Some("any")).await;
    let _dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");

    // Drop the WebSocket without sending step_complete. The server-side
    // step_frame timeout (30s) should fire and mark the job failed.
    drop(ws);

    // Allow up to 50s (timeout=30s + slack for finishing/event writes).
    let mut failed = false;
    for _ in 0..100 {
        if job_status(&app.pool, job_id).await == "failed" {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(failed, "job should fail within ~35s of mid-step disconnect");
}

#[tokio::test]
async fn coordinator_only_step_runs_locally() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_co").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    fake_worker_register(&mut ws, "fake_co", vec!["transcode".into()]).await;

    // Submit a `notify` step (CoordinatorOnly). Omit run_on -- the
    // parser rejects `run_on: any` on coordinator-only steps anyway.
    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "remote_coordinator_only", "notify", None).await;

    // The worker must not see step_dispatch. Wait briefly; the local
    // engine will fail the notify step (no notifier configured) but
    // never reaches out to the remote.
    let observed = wait_for_step_dispatch(&mut ws, Duration::from_secs(2)).await;
    assert!(
        observed.is_none(),
        "worker should not have received step_dispatch for coordinator-only step"
    );
}

#[tokio::test]
async fn no_eligible_workers_falls_back_to_local() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "fake_off").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    fake_worker_register(&mut ws, "fake_off", vec!["transcode".into()]).await;

    // Disable the remote worker so dispatch::route returns Local.
    let resp = client
        .patch(format!("{}/api/workers/{worker_id}", app.url))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "PATCH must succeed");

    // Give the dispatcher a moment to observe the disabled flag.
    tokio::time::sleep(Duration::from_millis(700)).await;

    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "remote_no_eligible", "transcode", Some("any")).await;

    // Must NOT see a step_dispatch on the WS.
    let observed = wait_for_step_dispatch(&mut ws, Duration::from_secs(2)).await;
    assert!(
        observed.is_none(),
        "disabled worker should not receive step_dispatch"
    );
}
