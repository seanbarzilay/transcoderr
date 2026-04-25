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
