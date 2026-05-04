//! Integration tests for `PATCH /api/workers/:id`. The rest of the
//! workers API surface is covered elsewhere (`worker_path_mappings_api`,
//! `worker_enroll`, `auto_discovery`, `worker_connect`) — this file is
//! the home for behaviour that's specific to the patch endpoint.

mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn patch_refuses_to_disable_local_worker() {
    // Disabling the seeded `kind='local'` row would silently stop the
    // coordinator from processing any jobs locally — there's no
    // scenario where this is useful, so the endpoint rejects it.
    let app = boot().await;
    let client = reqwest::Client::new();

    // id=1 is the seeded local row.
    let resp = client
        .patch(format!("{}/api/workers/1", app.url))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "kind='local' must be rejected with 400");

    // The row's enabled flag must be unchanged.
    let workers: serde_json::Value = client
        .get(format!("{}/api/workers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let local = workers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"].as_i64() == Some(1))
        .expect("seeded local row");
    assert_eq!(
        local["enabled"].as_bool(),
        Some(true),
        "local row must stay enabled after a rejected disable",
    );
}

#[tokio::test]
async fn patch_remote_worker_round_trips() {
    // Sanity: the same endpoint still works for remote workers.
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

    let resp = client
        .patch(format!("{}/api/workers/{}", app.url, id))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["enabled"].as_bool(), Some(false));
}
