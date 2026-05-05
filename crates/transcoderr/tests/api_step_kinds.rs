//! Integration test for `GET /api/step-kinds`. The endpoint is what
//! lets MCP-driven flow authoring discover the built-in catalog and
//! plugin step shapes without grepping source.

mod common;

use serial_test::serial;

async fn auth_token(app: &common::TestApp) -> String {
    use transcoderr::db::api_tokens;
    let made = api_tokens::create(&app.pool, "test").await.unwrap();
    made.token
}

#[tokio::test]
#[serial]
async fn step_kinds_returns_built_in_catalog_with_metadata() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/step-kinds", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = r.as_array().expect("response is an array");
    assert!(!arr.is_empty(), "step-kinds should not be empty after boot");

    // Sanity-check a few well-known built-ins are present with the
    // expected shape.
    let by_name: std::collections::HashMap<&str, &serde_json::Value> = arr
        .iter()
        .map(|v| (v["name"].as_str().unwrap(), v))
        .collect();

    let probe = by_name.get("probe").expect("probe should be registered");
    assert_eq!(probe["kind"], "builtin");
    assert_eq!(probe["executor"], "coordinator_only");

    // `transcode` is a remote-eligible built-in; executor should be "any".
    let transcode = by_name
        .get("transcode")
        .expect("transcode should be registered");
    assert_eq!(transcode["kind"], "builtin");
    assert_eq!(transcode["executor"], "any");

    // The output is sorted by name — adjacent compare.
    let names: Vec<&str> = arr.iter().map(|v| v["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "step-kinds response should be sorted by name");
}

