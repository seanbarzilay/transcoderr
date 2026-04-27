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

#[tokio::test]
async fn create_source_radarr_test_webhook_callback_succeeds() {
    // Servarr's POST /api/v3/notification synchronously test-fires the
    // configured webhook before returning. The mock here mimics that
    // behavior: when we receive the POST, call back to transcoderr's
    // /webhook/radarr endpoint with the configured Basic auth password,
    // and only return 200 if the callback succeeds.
    //
    // This test caught the original chicken-and-egg bug where the local
    // row hadn't been inserted yet when Servarr's test webhook arrived.
    let arr = MockServer::start().await;
    let app = common::boot().await;

    // The handshake mock: respond to POST /api/v3/notification by
    // probing transcoderr's webhook endpoint with the password we were
    // given. If transcoderr accepts (202), return 200 with id=99. If
    // transcoderr rejects (401), return 400 (matching real Radarr).
    let app_url = app.url.clone();
    Mock::given(method("POST"))
        .and(path("/api/v3/notification"))
        .respond_with(move |req: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let password = body["fields"]
                .as_array()
                .and_then(|fs| fs.iter().find(|f| f["name"] == "password"))
                .and_then(|f| f["value"].as_str())
                .unwrap_or("")
                .to_string();
            let url = body["fields"]
                .as_array()
                .and_then(|fs| fs.iter().find(|f| f["name"] == "url"))
                .and_then(|f| f["value"].as_str())
                .unwrap_or("")
                .to_string();
            // Override any URL the *arr received with the test app's
            // bound URL — TRANSCODERR_PUBLIC_URL inside boot() may be
            // a stub like http://test:8099 that wiremock can't resolve.
            let _ = url;
            let app_url = app_url.clone();

            // Block on the callback: if it 401s, the *arr would surface
            // a 400 with the test-failure JSON; if it 202s, the *arr
            // would return 200.
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let result = rt.block_on(async {
                    let client = reqwest::Client::new();
                    client
                        .post(format!("{app_url}/webhook/radarr"))
                        .basic_auth("", Some(&password))
                        .json(&serde_json::json!({"eventType": "Test"}))
                        .send()
                        .await
                        .map(|r| r.status().as_u16())
                        .unwrap_or(0)
                });
                let _ = tx.send(result);
            });
            let callback_status = rx.recv().unwrap_or(0);

            if callback_status == 202 || callback_status == 200 {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": 99,
                    "name": "transcoderr-Films",
                    "implementation": "Webhook",
                    "configContract": "WebhookSettings",
                    "fields": [],
                    "onDownload": true,
                }))
            } else {
                ResponseTemplate::new(400).set_body_json(serde_json::json!([{
                    "isWarning": false,
                    "propertyName": "Url",
                    "errorMessage": format!("test webhook failed: status {callback_status}"),
                    "severity": "error",
                }]))
            }
        })
        .expect(1)
        .mount(&arr)
        .await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "radarr",
            "name": "Films",
            "config": { "base_url": arr.uri(), "api_key": "k" },
            "secret_token": ""
        }))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        status, 200,
        "create returned {status} (test-webhook callback would have 401d if the row was not persisted before *arr was called); body={body}"
    );
    let id = body["id"].as_i64().unwrap();

    let detail: serde_json::Value = client
        .get(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["config"]["arr_notification_id"], 99);
}

#[tokio::test]
async fn create_source_radarr_rolls_back_local_row_when_arr_fails() {
    let arr = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/notification"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Unauthorized"
        })))
        .expect(1)
        .mount(&arr)
        .await;

    let app = common::boot().await;
    let client = reqwest::Client::new();

    let before: Vec<serde_json::Value> = client
        .get(format!("{}/api/sources", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let resp = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "radarr",
            "name": "Bad",
            "config": { "base_url": arr.uri(), "api_key": "wrong" },
            "secret_token": ""
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 502);

    let after: Vec<serde_json::Value> = client
        .get(format!("{}/api/sources", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        before.len(),
        "source count grew despite *arr failure — rollback didn't fire"
    );
    assert!(
        after.iter().all(|s| s["name"] != "Bad"),
        "the failed source row was left behind: {after:?}"
    );
}
