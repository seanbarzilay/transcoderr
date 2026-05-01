mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn plugin_catalogs_crud() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // The migration seeds one official catalog. List should show it.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list1.len(), 1);
    assert_eq!(list1[0]["name"], "transcoderr official");

    // Create a private catalog with an auth header.
    let resp: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "internal",
            "url": "https://internal.example/index.json",
            "auth_header": "Bearer xyz",
            "priority": 5,
        }))
        .send().await.unwrap().json().await.unwrap();
    let id = resp["id"].as_i64().unwrap();

    // List now has two.
    let list2: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list2.len(), 2);

    // Delete returns 204.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 204);

    // Deleting again returns 404.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn browse_returns_entries_and_errors_per_catalog() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = boot().await;
    let client = reqwest::Client::new();

    // Replace the seed catalog with a wiremock-backed one so the test
    // doesn't try to fetch from the real internet.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list1[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();

    let server_ok = MockServer::start().await;
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "size-report",
                "version": "0.1.0",
                "summary": "size",
                "tarball_url": "https://example.com/x.tgz",
                "tarball_sha256": "h",
                "kind": "subprocess",
                "provides_steps": ["size.report.before"]
            }]
        })))
        .mount(&server_ok).await;

    client.post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "ok",
            "url": format!("{}/index.json", server_ok.uri()),
        }))
        .send().await.unwrap();

    let body: serde_json::Value = client
        .get(format!("{}/api/plugin-catalog-entries", app.url))
        .send().await.unwrap().json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "size-report");
    assert_eq!(entries[0]["catalog_name"], "ok");
    assert!(body["errors"].as_array().unwrap().is_empty());
}
