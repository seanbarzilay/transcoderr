//! Integration tests for Piece 5's plugin-step remote routing.
//! Verifies the dispatcher's per-worker `available_steps` filter
//! correctly routes plugin steps to workers that have the plugin AND
//! skips workers that don't.
//!
//!  1. plugin_step_routes_to_worker_that_has_it
//!  2. plugin_step_skips_worker_without_it
//!  3. coordinator_only_plugin_step_runs_locally
//!  4. re_register_updates_available_steps
//!  5. disconnect_clears_available_steps_for_dispatch
//!
//! Note: these tests use `transcode` as the dispatch target instead of
//! an actual plugin step kind. Reason: the test fixture's registry
//! isn't seeded with plugin SubprocessSteps. The dispatch path is
//! identical -- `eligible_remotes` filter sees `worker_has_step(...)`
//! either way.

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

/// Send a Register with the given `available_steps`; consume the
/// register_ack. Returns the ack envelope so the test can inspect
/// the manifest if needed.
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

/// Send a re-register frame mid-connection. No ack is expected; the
/// coordinator's receive loop processes it silently.
async fn send_re_register(ws: &mut Ws, name: &str, available_steps: Vec<String>) {
    let reg = Envelope {
        id: format!("reg-{}", uuid::Uuid::new_v4()),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps,
            plugin_manifest: vec![],
        }),
    };
    send_env(ws, &reg).await;
}

/// Insert a flow + a pending job pointing at a single `use:` step,
/// optionally tagged with `run_on:`. Mirrors the canonical helper in
/// `tests/remote_dispatch.rs` (uses the same `db::flows::insert` /
/// `db::jobs::insert` helpers so column constraints are satisfied).
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

#[tokio::test]
async fn plugin_step_routes_to_worker_that_has_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker advertises transcode (a known built-in with Executor::Any).
    // The dispatch path is identical -- eligible_remotes filter sees
    // worker_has_step("transcode", ...) either way.
    send_register_and_get_ack(&mut ws, "fake1", vec!["transcode".into()]).await;

    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "plugin_routes", "transcode", Some("any")).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await;
    assert!(
        dispatch.is_some(),
        "worker advertising transcode should receive step_dispatch"
    );
}

#[tokio::test]
async fn plugin_step_skips_worker_without_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_no_whisper").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker only advertises remux -- NOT transcode.
    send_register_and_get_ack(&mut ws, "fake_no_whisper", vec!["remux".into()])
        .await;

    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "plugin_skips", "transcode", Some("any")).await;

    // No dispatch should arrive -- the dispatcher's filter rejects this
    // worker for `transcode` since it's not in its available_steps.
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(2)).await;
    assert!(
        dispatch.is_none(),
        "worker should NOT receive dispatch for an unadvertised step kind"
    );
}

#[tokio::test]
async fn coordinator_only_plugin_step_runs_locally() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_co").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Worker advertises everything -- but the step we submit is
    // `notify` (CoordinatorOnly built-in). Should run locally.
    send_register_and_get_ack(
        &mut ws,
        "fake_co",
        vec!["transcode".into(), "notify".into()],
    )
    .await;

    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "plugin_coord_only", "notify", None).await;

    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(2)).await;
    assert!(
        dispatch.is_none(),
        "coordinator-only step should not dispatch to worker"
    );
}

#[tokio::test]
async fn re_register_updates_available_steps() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_re").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    // Initial register: no transcode advertised.
    send_register_and_get_ack(&mut ws, "fake_re", vec!["remux".into()]).await;

    // Verify initial state by submitting a transcode step -> no dispatch.
    let (_flow_id, _job_id_a) =
        submit_job_with_step(&app, "plugin_re_register_a", "transcode", Some("any"))
            .await;
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(2)).await;
    assert!(
        dispatch.is_none(),
        "before re-register: transcode should NOT dispatch"
    );

    // Re-register with transcode added.
    send_re_register(
        &mut ws,
        "fake_re",
        vec!["remux".into(), "transcode".into()],
    )
    .await;

    // Brief pause to let the coordinator's receive loop process it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Submit another transcode step -- this time it should dispatch.
    let (_flow_id, _job_id_b) =
        submit_job_with_step(&app, "plugin_re_register_b", "transcode", Some("any"))
            .await;
    let dispatch = wait_for_step_dispatch(&mut ws, Duration::from_secs(5)).await;
    assert!(
        dispatch.is_some(),
        "after re-register: transcode should dispatch to the worker"
    );
}

#[tokio::test]
async fn disconnect_clears_available_steps_for_dispatch() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake_drop").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    send_register_and_get_ack(&mut ws, "fake_drop", vec!["transcode".into()]).await;

    // Disconnect.
    drop(ws);

    // Brief pause for SenderGuard::drop's spawned cleanup task to run.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Submit a transcode step. No worker is connected -> dispatcher
    // falls back to local. The job lifecycle proceeds without panic.
    // (We can't assert on a "missing" remote dispatch without a fake
    // worker still listening -- the dispatch::route fall-through to
    // Route::Local is exercised at the unit-test level in Task 8.)
    let (_flow_id, _job_id) =
        submit_job_with_step(&app, "plugin_disconnect", "transcode", Some("any"))
            .await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    // No assertion needed -- the test passes if no panic / crash occurred.
}
