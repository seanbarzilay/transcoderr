//! Integration tests for the unauthenticated `POST /api/worker/enroll`
//! endpoint. The endpoint is the server side of the worker
//! auto-discovery flow; the worker-side helper that calls it lives in
//! `worker::enroll` and is exercised end-to-end in `tests/auto_discovery.rs`.

mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn enroll_returns_token_and_inserts_row() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": "auto-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "enroll should succeed unauthenticated");

    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["id"].as_i64().unwrap();
    let token = body["secret_token"].as_str().unwrap();
    let ws_url = body["ws_url"].as_str().unwrap();
    assert!(id > 0, "id must be positive: {id}");
    assert_eq!(token.len(), 64, "token must be 32-byte hex (64 chars): {token}");
    assert!(ws_url.starts_with("ws://"), "ws_url must use ws:// scheme: {ws_url}");
    assert!(ws_url.ends_with("/api/worker/connect"), "ws_url must point at /api/worker/connect: {ws_url}");

    // The row must exist and be `kind='remote'`.
    let row = transcoderr::db::workers::get_by_id(&app.pool, id)
        .await
        .unwrap()
        .expect("row exists");
    assert_eq!(row.kind, "remote");
    assert_eq!(row.name, "auto-1");
    assert_eq!(row.secret_token.as_deref(), Some(token));
}

#[tokio::test]
async fn enroll_rejects_empty_name() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn enroll_does_not_require_authentication() {
    // The endpoint must work with NO Authorization header AND no
    // session cookie — that's the whole point of auto-enrollment.
    let app = boot().await;
    let client = reqwest::Client::builder()
        .cookie_store(false)
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": "no-auth"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
