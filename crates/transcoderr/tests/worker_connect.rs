//! Integration tests for the worker WS upgrade + register handshake.
//! Spins up the in-process axum router, connects a real WS client to
//! it, exercises the protocol end to end.

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Heartbeat, Message, PluginManifestEntry, Register,
};

/// Mint a remote-worker token via the REST endpoint and return it.
async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send().await.unwrap()
        .json().await.unwrap();
    let id = resp["id"].as_i64().expect("id");
    let token = resp["secret_token"].as_str().expect("token").to_string();
    (id, token)
}

/// Open a real WS connection to the in-process router with the given
/// Bearer token. URL is the running app's `ws://...` form.
async fn ws_connect(
    base_ws: &str,
    token: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    tokio_tungstenite::tungstenite::Error,
> {
    let url = format!("{base_ws}/api/worker/connect");
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    tokio_tungstenite::connect_async(req).await.map(|(s, _)| s)
}

fn make_register(name: &str) -> Envelope {
    Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({"encoders": []}),
            available_steps: vec!["plan.execute".into()],
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    }
}

async fn send_env(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    env: &Envelope,
) {
    let s = serde_json::to_string(env).unwrap();
    ws.send(WsMessage::Text(s)).await.unwrap();
}

async fn recv_env(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Envelope {
    let raw = ws.next().await.unwrap().unwrap();
    match raw {
        WsMessage::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_with_valid_token_succeeds_and_register_persists() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "gpu-box-1").await;

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await.unwrap();

    send_env(&mut ws, &make_register("gpu-box-1")).await;

    // Expect register_ack with the worker_id we just minted.
    let ack = recv_env(&mut ws).await;
    match ack.message {
        Message::RegisterAck(a) => assert_eq!(a.worker_id, worker_id),
        other => panic!("expected register_ack, got {other:?}"),
    }

    // DB should now have hw_caps_json + last_seen_at populated.
    let row: (Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT hw_caps_json, last_seen_at FROM workers WHERE id = ?",
    )
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(row.0.is_some(), "hw_caps_json must be persisted");
    assert!(row.1.is_some(), "last_seen_at must be set");
}

#[tokio::test]
async fn connect_with_invalid_token_fails() {
    let app = boot().await;
    let base_ws = app.url.replace("http://", "ws://");

    let result = ws_connect(&base_ws, "not-a-real-token").await;
    // Tungstenite reports the 401 as an Http error during handshake.
    assert!(result.is_err(), "connect with bogus token must fail");
}

#[tokio::test]
async fn heartbeat_keeps_last_seen_fresh() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (worker_id, token) = mint_token(&client, &app.url, "hb-box").await;
    let base_ws = app.url.replace("http://", "ws://");

    let mut ws = ws_connect(&base_ws, &token).await.unwrap();
    send_env(&mut ws, &make_register("hb-box")).await;
    let _ack = recv_env(&mut ws).await;

    // Capture initial last_seen.
    let initial: i64 = sqlx::query_as::<_, (i64,)>(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(worker_id)
    .fetch_one(&app.pool)
    .await
    .unwrap()
    .0;

    // Wait a real second so unix-second granularity advances.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    // Send a heartbeat.
    send_env(
        &mut ws,
        &Envelope {
            id: "hb-1".into(),
            message: Message::Heartbeat(Heartbeat {}),
        },
    )
    .await;

    // The websocket receive loop records heartbeats asynchronously. Poll with
    // a deadline so a busy CI runner does not fail just because it processed
    // the frame slightly after one fixed sleep.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut after = initial;
    while std::time::Instant::now() < deadline {
        after = sqlx::query_as::<_, (i64,)>(
            "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
        )
        .bind(worker_id)
        .fetch_one(&app.pool)
        .await
        .unwrap()
        .0;
        if after > initial {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert!(
        after > initial,
        "heartbeat must advance last_seen_at (was {initial}, now {after})"
    );
}

#[tokio::test]
async fn list_endpoint_redacts_secret_token_under_token_auth() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_worker_id, token) = mint_token(&client, &app.url, "redact-test").await;

    // Read /api/workers with the worker token as Bearer auth — auth.rs
    // accepts worker tokens (Task 7) and marks the call as Token-authed,
    // which triggers the redaction policy in api/workers.rs::list.
    let resp: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    let arr = resp.as_array().unwrap();
    let remote = arr.iter().find(|w| w["kind"] == "remote").unwrap();
    assert_eq!(remote["secret_token"], "***");
}
