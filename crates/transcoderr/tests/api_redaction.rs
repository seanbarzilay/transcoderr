mod common;
use common::boot;
use serde_json::json;
use transcoderr::{api::auth::hash_password, db};

#[tokio::test]
async fn token_authed_caller_sees_redacted_source_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();

    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    // Seed a source via SQL so we don't depend on the create endpoint here.
    sqlx::query("INSERT INTO sources (kind, name, config_json, secret_token) VALUES ('radarr','x','{}','sekrit')")
        .execute(&app.pool).await.unwrap();

    let listed: Vec<serde_json::Value> = reqwest::Client::new()
        .get(format!("{}/api/sources", app.url))
        .bearer_auth(&made.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["secret_token"], json!("***"));

    // Same call via session cookie returns the cleartext.
    let session = reqwest::Client::builder().cookie_store(true).build().unwrap();
    session.post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password":"hunter2"})).send().await.unwrap();
    let listed2: Vec<serde_json::Value> = session
        .get(format!("{}/api/sources", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(listed2[0]["secret_token"], json!("sekrit"));
}

#[tokio::test]
async fn token_authed_caller_sees_redacted_notifier_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();
    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    db::notifiers::upsert(
        &app.pool, "tg", "telegram",
        &json!({"bot_token": "1234:secret", "chat_id": "42"})
    ).await.unwrap();

    let listed: Vec<serde_json::Value> = reqwest::Client::new()
        .get(format!("{}/api/notifiers", app.url))
        .bearer_auth(&made.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["config"]["bot_token"], json!("***"));
    assert_eq!(listed[0]["config"]["chat_id"], json!("42"));
}

#[tokio::test]
async fn ntfy_topic_is_redacted_for_token_authed_caller() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();
    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    db::notifiers::upsert(
        &app.pool, "alerts", "ntfy",
        &json!({"server": "https://ntfy.sh", "topic": "secret-channel-name"})
    ).await.unwrap();

    let listed: Vec<serde_json::Value> = reqwest::Client::new()
        .get(format!("{}/api/notifiers", app.url))
        .bearer_auth(&made.token).send().await.unwrap().json().await.unwrap();
    assert_eq!(listed[0]["config"]["topic"], json!("***"));
    assert_eq!(listed[0]["config"]["server"], json!("https://ntfy.sh"));
}

#[tokio::test]
async fn put_source_with_redaction_sentinel_preserves_real_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();
    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO sources (kind, name, config_json, secret_token) VALUES ('radarr','x','{}','keep-me') RETURNING id"
    ).fetch_one(&app.pool).await.unwrap();

    // Token-authed PUT echoing back the redacted sentinel must NOT clobber the real secret.
    let resp = reqwest::Client::new()
        .put(format!("{}/api/sources/{}", app.url, id))
        .bearer_auth(&made.token)
        .json(&json!({"name": "renamed", "secret_token": "***"}))
        .send().await.unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    let after: String = sqlx::query_scalar("SELECT secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_one(&app.pool).await.unwrap();
    assert_eq!(after, "keep-me");

    // But explicit non-sentinel updates still go through.
    reqwest::Client::new()
        .put(format!("{}/api/sources/{}", app.url, id))
        .bearer_auth(&made.token)
        .json(&json!({"secret_token": "new-real-token"}))
        .send().await.unwrap();
    let final_secret: String = sqlx::query_scalar("SELECT secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_one(&app.pool).await.unwrap();
    assert_eq!(final_secret, "new-real-token");
}

#[tokio::test]
async fn put_notifier_with_redacted_bot_token_preserves_real_secret() {
    let app = boot().await;
    let h = hash_password("hunter2").unwrap();
    db::settings::set(&app.pool, "auth.enabled", "true").await.unwrap();
    db::settings::set(&app.pool, "auth.password_hash", &h).await.unwrap();
    let made = transcoderr::db::api_tokens::create(&app.pool, "mcp").await.unwrap();

    db::notifiers::upsert(
        &app.pool, "tg", "telegram",
        &json!({"bot_token": "real-secret", "chat_id": "42"})
    ).await.unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM notifiers WHERE name = 'tg'")
        .fetch_one(&app.pool).await.unwrap();

    // Token-authed PUT with bot_token redacted must keep the real one.
    let resp = reqwest::Client::new()
        .put(format!("{}/api/notifiers/{}", app.url, id))
        .bearer_auth(&made.token)
        .json(&json!({
            "name": "tg",
            "kind": "telegram",
            "config": {"bot_token": "***", "chat_id": "999"}
        }))
        .send().await.unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    // chat_id was actually updated; bot_token was preserved.
    let row: (String,) = sqlx::query_as("SELECT config_json FROM notifiers WHERE id = ?")
        .bind(id).fetch_one(&app.pool).await.unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&row.0).unwrap();
    assert_eq!(cfg["bot_token"], json!("real-secret"));
    assert_eq!(cfg["chat_id"], json!("999"));
}
