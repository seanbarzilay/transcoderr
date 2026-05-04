//! Integration tests for Piece 4's plugin push:
//!  1. tarball_endpoint_serves_cached_file
//!  2. tarball_endpoint_rejects_missing_token
//!  3. tarball_endpoint_404_for_unknown_plugin
//!  4. register_ack_carries_plugin_manifest
//!  5. plugin_install_broadcasts_plugin_sync
//!  6. plugin_uninstall_broadcasts_plugin_sync_without_it

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{Envelope, Message, PluginManifestEntry, Register};

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
        resp["id"].as_i64().unwrap(),
        resp["secret_token"].as_str().unwrap().to_string(),
    )
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let mut req = format!("{base_ws}/api/worker/connect")
        .as_str()
        .into_client_request()
        .unwrap();
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

async fn send_register_and_get_ack(ws: &mut Ws, name: &str) -> Envelope {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps: vec![],
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

/// Seed a plugin row + cache file directly via SQL/filesystem.
async fn seed_plugin(app: &common::TestApp, name: &str, sha: &str, body: &[u8]) {
    sqlx::query(
        "INSERT INTO plugins (name, version, kind, path, schema_json, enabled, tarball_sha256)
         VALUES (?, '1.0', 'subprocess', ?, '{}', 1, ?)",
    )
    .bind(name)
    .bind(format!("{}/plugins/{name}", app.data_dir.display()))
    .bind(sha)
    .execute(&app.pool)
    .await
    .unwrap();

    let cache = app.data_dir.join("plugins").join(".tarball-cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join(format!("{name}-{sha}.tar.gz")), body).unwrap();
}

async fn wait_for_plugin_sync(ws: &mut Ws, deadline: Duration) -> Option<Envelope> {
    let res = tokio::time::timeout(deadline, async {
        loop {
            let env = recv_env(ws).await;
            if matches!(env.message, Message::PluginSync(_)) {
                return env;
            }
        }
    })
    .await;
    res.ok()
}

#[tokio::test]
async fn tarball_endpoint_serves_cached_file() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "w1").await;

    let body: &[u8] = b"fake tarball body";
    let sha = "abc123def";
    seed_plugin(&app, "p1", sha, body).await;

    let resp = client
        .get(format!("{}/api/worker/plugins/p1/tarball", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(&bytes[..], body);
}

#[tokio::test]
async fn tarball_endpoint_rejects_missing_token() {
    let app = boot().await;
    seed_plugin(&app, "p1", "abc", b"x").await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/worker/plugins/p1/tarball", app.url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn tarball_endpoint_404_for_unknown_plugin() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "w1").await;

    let resp = client
        .get(format!("{}/api/worker/plugins/nope/tarball", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn register_ack_carries_plugin_manifest() {
    let app = boot().await;
    seed_plugin(&app, "size-report", "deadbeef", b"x").await;

    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    let ack = send_register_and_get_ack(&mut ws, "fake1").await;
    let plugin_install = match ack.message {
        Message::RegisterAck(a) => a.plugin_install,
        _ => panic!("expected register_ack"),
    };
    assert_eq!(plugin_install.len(), 1);
    assert_eq!(plugin_install[0].name, "size-report");
    assert_eq!(plugin_install[0].sha256, "deadbeef");
    assert!(plugin_install[0]
        .tarball_url
        .contains("/api/worker/plugins/size-report/tarball"));
}

#[tokio::test]
async fn plugin_install_broadcasts_plugin_sync() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    let _ack = send_register_and_get_ack(&mut ws, "fake1").await;

    // Seed plugin + trigger broadcast (simulates what api/plugins.rs::install does).
    seed_plugin(&app, "extra", "xxx", b"x").await;
    transcoderr::api::plugins::broadcast_manifest_for_test(&app.state).await;

    let env = wait_for_plugin_sync(&mut ws, Duration::from_secs(2))
        .await
        .expect("worker should receive plugin_sync within 2s");
    let plugins = match env.message {
        Message::PluginSync(p) => p.plugins,
        _ => unreachable!(),
    };
    assert!(plugins.iter().any(|p| p.name == "extra"));
}

#[tokio::test]
async fn plugin_uninstall_broadcasts_plugin_sync_without_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;

    seed_plugin(&app, "going-away", "yyy", b"x").await;

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    let _ack = send_register_and_get_ack(&mut ws, "fake1").await;

    // Remove the row + cache file directly, then re-broadcast.
    sqlx::query("DELETE FROM plugins WHERE name = 'going-away'")
        .execute(&app.pool)
        .await
        .unwrap();
    let cache = app.data_dir.join("plugins").join(".tarball-cache");
    let _ = std::fs::remove_file(cache.join("going-away-yyy.tar.gz"));
    transcoderr::api::plugins::broadcast_manifest_for_test(&app.state).await;

    let env = wait_for_plugin_sync(&mut ws, Duration::from_secs(2))
        .await
        .expect("worker should receive plugin_sync within 2s");
    let plugins = match env.message {
        Message::PluginSync(p) => p.plugins,
        _ => unreachable!(),
    };
    assert!(plugins.iter().all(|p| p.name != "going-away"));
}
