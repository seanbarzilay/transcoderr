//! End-to-end: per-worker path mappings rewrite paths on the wire in
//! both directions. Boot a coordinator, register a fake worker,
//! configure path_mappings_json for that worker, dispatch a step with
//! `/coord/...` path. Assert the worker sees `/worker/...`. Worker
//! replies with worker-space paths in ctx.steps.transcode.output_path;
//! assert the coordinator's restored ctx has coordinator-space paths.

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
    Envelope, Message, PluginManifestEntry, Register, StepComplete,
};

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        resp["id"].as_i64().expect("id"),
        resp["secret_token"]
            .as_str()
            .expect("secret_token")
            .to_string(),
    )
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let url = format!("{base_ws}/api/worker/connect");
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
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

async fn send_register_and_drain_ack(ws: &mut Ws, name: &str) {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({"encoders": []}),
            available_steps: vec!["transcode".into()],
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    let _ack = recv_env(ws).await;
}

/// Insert a flow + pending job. Returns (flow_id, job_id).
async fn submit_job_with_step(
    app: &common::TestApp,
    flow_name: &str,
    use_: &str,
    file_path: &str,
) -> (i64, i64) {
    let yaml = format!(
        "name: {flow_name}\ntriggers: [{{ webhook: x }}]\nsteps:\n  - use: {use_}\n    run_on: any\n"
    );
    let flow = parse_flow(&yaml).unwrap();
    let flow_id = db::flows::insert(&app.pool, flow_name, &yaml, &flow)
        .await
        .unwrap();
    let job_id = db::jobs::insert(&app.pool, flow_id, 1, "webhook", file_path, "{}")
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

#[tokio::test]
async fn round_trip_rewrites_paths_in_both_directions() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "fake_pm").await;

    // Configure path mappings for this worker BEFORE the WS connect, so
    // the cache fill on first dispatch picks them up.
    transcoderr::db::workers::update_path_mappings(
        &app.pool,
        worker_id,
        Some(r#"[{"from":"/coord","to":"/worker"}]"#),
    )
    .await
    .unwrap();

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    send_register_and_drain_ack(&mut ws, "fake_pm").await;

    // Submit a job with a /coord/... path; the engine will dispatch
    // a transcode step to the only eligible remote worker (us).
    let (_flow_id, job_id) =
        submit_job_with_step(&app, "pm_round_trip", "transcode", "/coord/movies/X.mkv").await;

    // 1. Forward rewrite: assert the worker sees the /worker/ path.
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(10))
        .await
        .expect("worker should receive step_dispatch within 10s");

    let dispatched_ctx = match &dispatch.message {
        Message::StepDispatch(d) => d.ctx_snapshot.clone(),
        _ => unreachable!(),
    };
    let dispatched_value: serde_json::Value = serde_json::from_str(&dispatched_ctx).unwrap();
    assert_eq!(
        dispatched_value["file"]["path"].as_str().unwrap(),
        "/worker/movies/X.mkv",
        "forward rewrite must map /coord -> /worker"
    );

    // 2. Worker replies with a NEW path inside ctx.steps.transcode.output_path
    //    in worker-space.
    let mut returned_ctx = dispatched_value.clone();
    returned_ctx["steps"] = json!({
        "transcode": {
            "output_path": "/worker/movies/X.transcoded.mkv"
        }
    });

    let correlation_id = dispatch.id.clone();
    let step_id_str = match dispatch.message {
        Message::StepDispatch(d) => d.step_id,
        _ => unreachable!(),
    };
    let complete = Envelope {
        id: correlation_id,
        message: Message::StepComplete(StepComplete {
            job_id,
            step_id: step_id_str,
            status: "ok".into(),
            error: None,
            ctx_snapshot: Some(returned_ctx.to_string()),
        }),
    };
    send_env(&mut ws, &complete).await;

    // 3. Reverse rewrite: poll until the job completes, then read the
    //    checkpoint's context_snapshot_json and assert coordinator-space
    //    paths are restored.
    let start = std::time::Instant::now();
    let final_status: Option<String> = loop {
        let status: Option<String> = sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?")
            .bind(job_id)
            .fetch_optional(&app.pool)
            .await
            .unwrap();
        if let Some(ref s) = status {
            if s == "completed" {
                break Some(s.clone());
            }
        }
        if start.elapsed() >= Duration::from_secs(5) {
            break status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        final_status.as_deref(),
        Some("completed"),
        "job should complete after StepComplete"
    );

    // Read the final context snapshot from the checkpoints table.
    let snapshot: Option<String> =
        sqlx::query_scalar("SELECT context_snapshot_json FROM checkpoints WHERE job_id = ?")
            .bind(job_id)
            .fetch_optional(&app.pool)
            .await
            .unwrap();
    let snapshot = snapshot.expect("checkpoint row must exist after step completes");
    let restored: serde_json::Value = serde_json::from_str(&snapshot).unwrap();

    assert_eq!(
        restored["file"]["path"].as_str().unwrap(),
        "/coord/movies/X.mkv",
        "reverse rewrite must map /worker -> /coord on file.path"
    );
    assert_eq!(
        restored["steps"]["transcode"]["output_path"]
            .as_str()
            .unwrap(),
        "/coord/movies/X.transcoded.mkv",
        "reverse rewrite must map /worker -> /coord inside ctx.steps too"
    );
}
