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
