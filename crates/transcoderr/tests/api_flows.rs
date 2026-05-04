mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn flows_crud_round_trip() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // empty list
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/flows", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());

    let yaml = r#"
name: t
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
"#;
    let created: serde_json::Value = client
        .post(format!("{}/api/flows", app.url))
        .json(&json!({"name":"t","yaml":yaml}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["version"], 1);

    let detail: serde_json::Value = client
        .get(format!("{}/api/flows/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["yaml_source"].as_str().unwrap(), yaml);

    let yaml2 = r#"
name: t
triggers:
  - radarr: [downloaded, upgraded]
steps:
  - use: probe
"#;
    let upd = client
        .put(format!("{}/api/flows/{id}", app.url))
        .json(&json!({"yaml":yaml2,"enabled":true}))
        .send()
        .await
        .unwrap();
    assert!(upd.status().is_success());

    let detail2: serde_json::Value = client
        .get(format!("{}/api/flows/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail2["version"], 2);
}
