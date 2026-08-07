//! Regression tests for the worker-token privilege-escalation bypass.
//!
//! `POST /api/worker/enroll` is unauthenticated by design (open LAN
//! enrollment). `require_auth` used to accept the `secret_token` it
//! mints as a valid Bearer credential on *every* protected route, with
//! no path check — despite a comment claiming the grant was scoped to
//! the worker paths. That combination let any unauthenticated caller
//! who could reach the HTTP port take over the entire API in two
//! requests, even with `auth.enabled = "true"`:
//!
//!     POST /api/worker/enroll  {"name":"x"}      -> {"secret_token": "..."}
//!     GET  /api/settings       Bearer <token>    -> 200 + auth.password_hash
//!
//! These tests pin the fix: a worker token authenticates the
//! `/worker/...` daemon surface and nothing else.

mod common;

use common::boot;
use serde_json::json;

/// Turn on auth the same way the Settings UI does.
async fn enable_auth(pool: &sqlx::SqlitePool) {
    let hash = transcoderr::api::auth::hash_password("correct horse battery staple").unwrap();
    transcoderr::db::settings::set(pool, "auth.password_hash", &hash)
        .await
        .unwrap();
    transcoderr::db::settings::set(pool, "auth.enabled", "true")
        .await
        .unwrap();
}

/// Enroll a worker with no credentials and return its secret token.
async fn enroll_unauthenticated(client: &reqwest::Client, url: &str) -> String {
    let resp = client
        .post(format!("{url}/api/worker/enroll"))
        .json(&json!({"name": "rogue"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "enrollment is intentionally unauthenticated; that is the premise of this test"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    body["secret_token"].as_str().unwrap().to_string()
}

fn client() -> reqwest::Client {
    // No cookie store: the only credential in play is the Bearer.
    reqwest::Client::builder()
        .cookie_store(false)
        .build()
        .unwrap()
}

#[tokio::test]
async fn worker_token_cannot_read_settings() {
    let app = boot().await;
    enable_auth(&app.pool).await;
    let client = client();
    let token = enroll_unauthenticated(&client, &app.url).await;

    let resp = client
        .get(format!("{}/api/settings", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "a worker token must not read /api/settings — that response contains auth.password_hash"
    );
}

#[tokio::test]
async fn worker_token_cannot_write_settings() {
    let app = boot().await;
    enable_auth(&app.pool).await;
    let client = client();
    let token = enroll_unauthenticated(&client, &app.url).await;

    let resp = client
        .patch(format!("{}/api/settings", app.url))
        .bearer_auth(&token)
        .json(&json!({"auth.enabled": "false"}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "a worker token must not be able to switch auth off"
    );

    // And auth really is still on.
    let still_on = transcoderr::db::settings::get(&app.pool, "auth.enabled")
        .await
        .unwrap();
    assert_eq!(still_on.as_deref(), Some("true"));
}

#[tokio::test]
async fn worker_token_cannot_reach_the_operator_worker_routes() {
    // `/api/workers` is the operator surface: it lists every worker row,
    // and PATCH returns another worker's cleartext secret_token. The
    // daemon surface is `/api/worker/...` (singular).
    let app = boot().await;
    enable_auth(&app.pool).await;
    let client = client();
    let token = enroll_unauthenticated(&client, &app.url).await;

    for path in ["/api/workers", "/api/flows", "/api/sources", "/api/plugins"] {
        let resp = client
            .get(format!("{}{}", app.url, path))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "{path} must reject a worker token when auth is enabled"
        );
    }
}

#[tokio::test]
async fn worker_token_still_authenticates_the_daemon_surface() {
    // The fix must not break real workers. `/api/worker/plugins/:name/tarball`
    // verifies the token itself; a valid token should get past auth and
    // fail on the missing plugin (404), never on the credential (401).
    let app = boot().await;
    enable_auth(&app.pool).await;
    let client = client();
    let token = enroll_unauthenticated(&client, &app.url).await;

    let resp = client
        .get(format!(
            "{}/api/worker/plugins/nonexistent/tarball",
            app.url
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        401,
        "a valid worker token must still authenticate the worker daemon surface"
    );
}

#[tokio::test]
async fn garbage_bearer_is_still_rejected() {
    // Baseline: the 401s above must come from the scope check, not from
    // every Bearer being rejected outright.
    let app = boot().await;
    enable_auth(&app.pool).await;
    let client = client();

    let resp = client
        .get(format!("{}/api/settings", app.url))
        .bearer_auth("not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}
