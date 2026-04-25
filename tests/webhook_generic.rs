mod common;

use common::boot;
use serde_json::json;
use std::time::Duration;
use transcoderr::{db, flow::parse_flow};

#[tokio::test]
async fn generic_webhook_creates_job_with_extracted_path() {
    let app = boot().await;

    // Insert a webhook source named "my-source" with path_expr config.
    db::sources::insert(
        &app.pool,
        "webhook",
        "my-source",
        &json!({ "path_expr": "steps.payload.path" }),
        "t",
    )
    .await
    .unwrap();

    // Seed a flow triggered by the "my-source" webhook.
    let yaml = r#"
name: generic-e2e
triggers: [{ webhook: my-source }]
steps:
  - id: probe
    use: probe
"#;
    let flow = parse_flow(yaml).unwrap();
    db::flows::insert(&app.pool, "generic-e2e", yaml, &flow)
        .await
        .unwrap();

    // POST to /webhook/my-source with a payload containing `path`.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/webhook/my-source", app.url))
        .bearer_auth("t")
        .json(&json!({ "path": "/some/file.mkv" }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    // Poll until the job row appears with the correct file_path.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT file_path FROM jobs WHERE source_kind = 'webhook' LIMIT 1")
                .fetch_optional(&app.pool)
                .await
                .unwrap();
        if let Some((file_path,)) = row {
            assert_eq!(file_path, "/some/file.mkv");
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("generic webhook job was not inserted in time");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
