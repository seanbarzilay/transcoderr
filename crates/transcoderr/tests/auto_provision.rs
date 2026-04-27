//! Integration test for the auto-provision create-source flow. Spins
//! up wiremock as a fake Radarr; confirms transcoderr POSTs to
//! /api/v3/notification before persisting the source row.

mod common;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn create_source_radarr_calls_arr_then_persists() {
    let arr = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/notification"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 42,
            "name": "transcoderr-Movies",
            "implementation": "Webhook",
            "configContract": "WebhookSettings",
            "fields": [],
            "onDownload": true,
        })))
        .expect(1)
        .mount(&arr)
        .await;

    let app = common::boot().await;
    let client = reqwest::Client::new();

    let resp: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "radarr",
            "name": "Movies",
            "config": {
                "base_url": arr.uri(),
                "api_key": "test-key",
            },
            "secret_token": ""
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let id = resp["id"].as_i64().unwrap();

    let detail: serde_json::Value = client
        .get(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["kind"], "radarr");
    assert_eq!(detail["name"], "Movies");
    assert_eq!(detail["config"]["arr_notification_id"], 42);
}
