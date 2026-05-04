mod common;

use common::boot;
use serde_json::json;
use std::time::Duration;
use transcoderr::{db, flow::parse_flow};

#[tokio::test]
async fn sonarr_webhook_creates_job() {
    let app = boot().await;

    // Insert a sonarr source.
    db::sources::insert(
        &app.pool,
        "sonarr",
        "sonarr-main",
        &json!({}),
        "sonarr-token",
    )
    .await
    .unwrap();

    // Seed a flow triggered by sonarr downloaded event.
    let yaml = r#"
name: sonarr-e2e
triggers: [{ sonarr: [downloaded] }]
steps:
  - id: probe
    use: probe
"#;
    let flow = parse_flow(yaml).unwrap();
    db::flows::insert(&app.pool, "sonarr-e2e", yaml, &flow)
        .await
        .unwrap();

    // POST a Sonarr-shaped payload.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/webhook/sonarr", app.url))
        .bearer_auth("sonarr-token")
        .json(&json!({
            "eventType": "Download",
            "episodeFile": { "path": "/tv/Show/S01E01.mkv" }
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    // Poll for job creation (don't wait for completion — no real file).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT source_kind FROM jobs WHERE source_kind = 'sonarr' LIMIT 1")
                .fetch_optional(&app.pool)
                .await
                .unwrap();
        if row.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("sonarr job was not inserted in time");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
