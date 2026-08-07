//! Regression tests for `PATCH /api/settings` writing the `auth.` namespace.
//!
//! The handler used to loop over every key in the request body and write
//! it straight to the `settings` table, excluding only the literal
//! `"auth.password"`. Nothing stopped a caller from setting
//! `auth.password_hash` (the value `login` verifies against) or
//! `auth.enabled` (the flag `require_auth` reads), so any caller that
//! reached the handler could install a password of their own choosing or
//! turn authentication off:
//!
//!     PATCH /api/settings {"auth.password_hash": "$argon2id$...<theirs>"}
//!     PATCH /api/settings {"auth.enabled": "false"}
//!
//! The same blind write-back is why a password change from the Settings
//! page silently reverted: the UI echoed the stale `auth.password_hash`
//! it had read from `GET /api/settings`, and the loop wrote it over the
//! freshly computed hash.

mod common;

use common::boot;
use serde_json::json;

async fn stored(pool: &sqlx::SqlitePool, key: &str) -> Option<String> {
    transcoderr::db::settings::get(pool, key).await.unwrap()
}

/// Tests start with auth disabled (the migration default), so no
/// credential is needed to reach the handler.
fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

#[tokio::test]
async fn patch_cannot_overwrite_the_password_hash() {
    let app = boot().await;
    let real = transcoderr::api::auth::hash_password("the real password").unwrap();
    transcoderr::db::settings::set(&app.pool, "auth.password_hash", &real)
        .await
        .unwrap();

    let resp = client()
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.password_hash": "$argon2id$v=19$m=19456,t=2,p=1$AAAA$BBBB"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    assert_eq!(
        stored(&app.pool, "auth.password_hash").await.as_deref(),
        Some(real.as_str()),
        "auth.password_hash must not be settable through PATCH /api/settings"
    );
}

#[tokio::test]
async fn patch_cannot_flip_auth_enabled_through_the_generic_loop() {
    // `auth.enabled` is only ever written from the explicit branch, and
    // turning it on requires a password. A body that tries to set it to
    // "true" without one must not leave it enabled with no credential.
    let app = boot().await;

    let resp = client()
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.enabled": "true"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "enabling auth without a password must be rejected"
    );
    assert_ne!(
        stored(&app.pool, "auth.enabled").await.as_deref(),
        Some("true"),
        "a rejected request must not have enabled auth"
    );
}

#[tokio::test]
async fn enabling_auth_sets_both_keys_and_the_password_works() {
    // The legitimate transition must still work end to end.
    let app = boot().await;
    let client = client();

    let resp = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.enabled": "true", "auth.password": "hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    assert_eq!(
        stored(&app.pool, "auth.enabled").await.as_deref(),
        Some("true")
    );
    let hash = stored(&app.pool, "auth.password_hash").await.unwrap();
    assert!(
        hash.starts_with("$argon2"),
        "expected an argon2 PHC string, got {hash}"
    );
    // The raw password must never be stored.
    assert!(stored(&app.pool, "auth.password").await.is_none());

    // And it actually authenticates.
    let resp = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "hunter2"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "the new password must log in");

    let resp = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "wrong"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn a_stale_password_hash_echo_does_not_revert_a_password_change() {
    // This is the Settings-page bug: the UI read the settings map (which
    // included auth.password_hash), typed a new password, and PATCHed the
    // whole draft back. The generic loop then wrote the stale hash over
    // the newly computed one and the password change silently reverted.
    let app = boot().await;
    // Auth gets enabled partway through, so this client needs to carry
    // the session cookie for the second PATCH.
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    // Establish an initial password (still unauthenticated at this point:
    // auth.enabled defaults to "false").
    client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.enabled": "true", "auth.password": "old-password"}))
        .send()
        .await
        .unwrap();
    let old_hash = stored(&app.pool, "auth.password_hash").await.unwrap();

    // Auth is on now — log in so the rotation PATCH is authorised.
    let resp = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "old-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "login with the initial password");

    // Rotate it, echoing the stale hash back exactly as the UI did.
    let resp = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({
            "auth.enabled": "true",
            "auth.password": "new-password",
            "auth.password_hash": old_hash,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let now = stored(&app.pool, "auth.password_hash").await.unwrap();
    assert_ne!(
        now, old_hash,
        "the echoed stale hash must not overwrite the new one"
    );

    let resp = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "new-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204, "the rotated password must log in");

    let resp = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "old-password"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "the old password must stop working");
}

#[tokio::test]
async fn ordinary_settings_are_still_writable() {
    // The filter must be limited to the `auth.` namespace.
    let app = boot().await;

    let resp = client()
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"retention.jobs_days": "7", "runs.max_concurrent": 4}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    assert_eq!(
        stored(&app.pool, "retention.jobs_days").await.as_deref(),
        Some("7")
    );
    assert_eq!(
        stored(&app.pool, "runs.max_concurrent").await.as_deref(),
        Some("4")
    );
}
