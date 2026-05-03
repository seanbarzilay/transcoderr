//! Integration tests for the `PUT /api/workers/:id/path-mappings`
//! endpoint. The endpoint refuses kind='local' rows, validates
//! non-empty from/to, normalises trailing slashes, and persists via
//! `db::workers::update_path_mappings`. Two more tests that assert
//! against the GET response shape (`path_mappings` field) live in
//! Task 5 — they are appended to this same file.

mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn put_path_mappings_echoes_canonical_rules() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    let put_resp = client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({
            "rules": [
                {"from": "/mnt/movies/", "to": "/data/media/movies/"},
                {"from": "/mnt/tv",      "to": "/data/media/tv"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(put_resp.status(), 200);
    let body: serde_json::Value = put_resp.json().await.unwrap();
    assert_eq!(body["id"].as_i64().unwrap(), id);
    let rules = body["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    // Trailing slashes stripped on save (canonicalised before echo).
    assert!(rules.iter().any(|r| r["from"].as_str() == Some("/mnt/movies")));
    assert!(rules.iter().any(|r| r["from"].as_str() == Some("/mnt/tv")));
}

#[tokio::test]
async fn put_path_mappings_refuses_local_worker() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // id=1 is the seeded local worker row.
    let resp = client
        .put(format!("{}/api/workers/1/path-mappings", app.url))
        .json(&json!({"rules": [{"from": "/a", "to": "/b"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "kind='local' must be rejected with 400");
}

#[tokio::test]
async fn put_path_mappings_rejects_empty_from_or_to() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    for body in &[
        json!({"rules": [{"from": "",     "to": "/b"}]}),
        json!({"rules": [{"from": "/a",   "to": ""}]}),
        json!({"rules": [{"from": "   ",  "to": "/b"}]}),
    ] {
        let resp = client
            .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
            .json(body)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "rejected: {body}");
    }
}

#[tokio::test]
async fn put_round_trips_via_get() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({
            "rules": [{"from": "/mnt/movies", "to": "/data/media/movies"}]
        }))
        .send()
        .await
        .unwrap();

    let workers: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = workers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_i64() == Some(id))
        .expect("worker row");
    let rules = row["path_mappings"].as_array().expect("path_mappings present");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["from"].as_str().unwrap(), "/mnt/movies");
    assert_eq!(rules[0]["to"].as_str().unwrap(), "/data/media/movies");
}

#[tokio::test]
async fn put_empty_rules_clears_to_null_in_get() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let create_resp: serde_json::Value = client
        .post(format!("{}/api/workers", app.url))
        .json(&json!({"name": "gpu-1"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_resp["id"].as_i64().unwrap();

    client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({"rules": [{"from": "/a", "to": "/b"}]}))
        .send()
        .await
        .unwrap();
    let resp = client
        .put(format!("{}/api/workers/{}/path-mappings", app.url, id))
        .json(&json!({"rules": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let workers: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = workers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_i64() == Some(id))
        .expect("worker row");
    assert!(row["path_mappings"].is_null(),
        "empty rules → DB NULL → JSON null");
}
