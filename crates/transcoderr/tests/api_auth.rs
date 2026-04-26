mod common;
use common::boot;
use serde_json::json;
use transcoderr::{api::auth::hash_password, db};

#[tokio::test]
async fn login_with_correct_password_succeeds() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let bad = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"wrong"})).send().await.unwrap();
    assert_eq!(bad.status(), 401);

    let ok = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();
    assert!(ok.status().is_success());

    let me: serde_json::Value = client.get(format!("{}/api/auth/me", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(me["authed"], true);
}

#[tokio::test]
async fn api_tokens_table_round_trips() {
    let app = boot().await;
    sqlx::query("INSERT INTO api_tokens (name, hash, prefix, created_at) VALUES (?, ?, ?, ?)")
        .bind("claude-desktop")
        .bind("$argon2id$dummy")
        .bind("tcr_a1b2c3d4")
        .bind(123_i64)
        .execute(&app.pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_tokens")
        .fetch_one(&app.pool).await.unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn api_token_create_verify_delete_round_trip() {
    use transcoderr::db::api_tokens;
    let app = boot().await;

    let made = api_tokens::create(&app.pool, "claude-desktop").await.unwrap();
    assert!(made.token.starts_with("tcr_"));
    assert_eq!(made.token.len(), 4 + 32);

    let id = api_tokens::verify(&app.pool, &made.token).await.expect("verify");
    assert_eq!(id, made.id);

    let bad = api_tokens::verify(&app.pool, "tcr_NOTREAL000000000000000000000000").await;
    assert!(bad.is_none());

    let listed = api_tokens::list(&app.pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].prefix.len(), 12);

    let removed = api_tokens::delete(&app.pool, made.id).await.unwrap();
    assert!(removed);

    let after = api_tokens::verify(&app.pool, &made.token).await;
    assert!(after.is_none());
}

#[tokio::test]
async fn token_endpoints_create_list_delete() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let client = reqwest::Client::builder().cookie_store(true).build().unwrap();
    let _ = client.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();

    // Create
    let made: serde_json::Value = client.post(format!("{}/api/auth/tokens", app.url))
        .json(&json!({"name":"claude-desktop"}))
        .send().await.unwrap()
        .json().await.unwrap();
    let token = made["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("tcr_"));

    // List
    let listed: Vec<serde_json::Value> = client.get(format!("{}/api/auth/tokens", app.url))
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].get("token").is_some(), "list must NOT include the secret");

    // Delete
    let id = made["id"].as_i64().unwrap();
    let del = client.delete(format!("{}/api/auth/tokens/{id}", app.url))
        .send().await.unwrap();
    assert!(del.status().is_success());

    let listed2: Vec<serde_json::Value> = client.get(format!("{}/api/auth/tokens", app.url))
        .send().await.unwrap()
        .json().await.unwrap();
    assert!(listed2.is_empty());
}

#[tokio::test]
async fn bearer_token_authenticates_to_protected_endpoint() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let made = transcoderr::db::api_tokens::create(&app.pool, "test").await.unwrap();

    // No auth → 401
    let r0 = reqwest::Client::new().get(format!("{}/api/flows", app.url)).send().await.unwrap();
    assert_eq!(r0.status(), 401);

    // Wrong token → 401
    let r1 = reqwest::Client::new().get(format!("{}/api/flows", app.url))
        .bearer_auth("tcr_definitelynotreal000000000000000")
        .send().await.unwrap();
    assert_eq!(r1.status(), 401);

    // Correct token → 200
    let r2 = reqwest::Client::new().get(format!("{}/api/flows", app.url))
        .bearer_auth(&made.token)
        .send().await.unwrap();
    assert!(r2.status().is_success(), "got {}", r2.status());
}
