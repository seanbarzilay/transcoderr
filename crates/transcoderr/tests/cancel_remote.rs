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
use transcoderr::db;
use transcoderr::flow::parse_flow;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register, StepComplete, StepCancelMsg,
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
    recv_env(ws).await
}

/// Insert a flow + pending job. Returns (flow_id, job_id).
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

/// Drain envelopes until a `step_dispatch` arrives or the deadline fires.
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

/// Drain envelopes until a `step_cancel` arrives or the deadline fires.
async fn wait_for_step_cancel(ws: &mut Ws, deadline: Duration) -> Option<Envelope> {
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

/// Poll the DB until the job reaches `target` status or `deadline` elapses.
async fn wait_for_job_status(
    pool: &sqlx::SqlitePool,
    job_id: i64,
    target: &str,
    deadline: Duration,
) -> Option<String> {
    let start = std::time::Instant::now();
    loop {
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
                .bind(job_id)
                .fetch_optional(pool)
                .await
                .unwrap();
        if let Some(ref s) = status {
            if s == target {
                return status;
            }
        }
        if start.elapsed() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Test 1: cancelling a job while a remote worker holds the step causes a
/// StepCancel envelope to arrive on the WS, correlated to the original
/// StepDispatch, and the job ultimately reaches `cancelled` status.
#[tokio::test]
async fn cancel_propagates_to_remote_worker() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_cancel").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    send_register_and_get_ack(&mut ws, "fake_cancel", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "cancel_propagates", "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");

    assert!(
        app.state.cancellations.cancel(job_id),
        "cancel should find the registered token"
    );

    let cancel_env = wait_for_step_cancel(&mut ws, Duration::from_secs(2))
        .await
        .expect("worker should receive step_cancel within 2s");

    assert_eq!(
        cancel_env.id, dispatch.id,
        "step_cancel correlation_id must match step_dispatch"
    );
    match cancel_env.message {
        Message::StepCancel(StepCancelMsg { job_id: cjid, .. }) => {
            assert_eq!(cjid, job_id);
        }
        other => panic!("expected StepCancel, got {other:?}"),
    }

    let final_status = wait_for_job_status(
        &app.pool,
        job_id,
        "cancelled",
        Duration::from_secs(3),
    )
    .await;
    assert_eq!(
        final_status.as_deref(),
        Some("cancelled"),
        "job should reach cancelled status within 3s (got {final_status:?})"
    );
}

/// Test 2: after triggering cancel, the job reaches `cancelled` status well
/// within 2s — confirming the cancel arm unblocks the engine promptly rather
/// than waiting for the 30s frame timeout.
#[tokio::test]
async fn cancel_unblocks_engine_within_a_second() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_fast").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_get_ack(&mut ws, "fake_fast", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "cancel_fast", "transcode", Some("any")).await;

    let _dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");

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

/// Test 3: calling cancel after the step has already completed (and the worker
/// has unregistered the token) is a no-op — no StepCancel envelope arrives at
/// the fake worker.
#[tokio::test]
async fn cancel_after_step_complete_is_silent() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_done").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_get_ack(&mut ws, "fake_done", vec!["transcode".into()]).await;

    let (_flow_id, job_id) =
        submit_job_with_step(&app, "cancel_after_done", "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");
    let correlation_id = dispatch.id.clone();
    let (step_id, dispatched_ctx) = match dispatch.message {
        Message::StepDispatch(d) => (d.step_id, d.ctx_snapshot),
        _ => unreachable!(),
    };

    // Complete the step successfully.
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

    // Wait for the engine to process the completion.
    let _ = wait_for_job_status(&app.pool, job_id, "completed", Duration::from_secs(5)).await;

    // Now cancel — the token should already be unregistered.
    let _ = app.state.cancellations.cancel(job_id);

    // No StepCancel should arrive.
    let cancel_env = wait_for_step_cancel(&mut ws, Duration::from_secs(1)).await;
    assert!(
        cancel_env.is_none(),
        "no step_cancel envelope should arrive after step_complete"
    );
}
