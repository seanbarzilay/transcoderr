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
