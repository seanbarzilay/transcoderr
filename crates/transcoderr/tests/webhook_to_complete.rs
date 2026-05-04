mod common;

use common::boot;
use serde_json::{json, json as json_macro};
use std::time::Duration;
use transcoderr::{db, ffmpeg::make_testsrc_mkv, flow::parse_flow};

#[tokio::test]
async fn radarr_webhook_drives_a_run_to_completion() {
    let app = boot().await;
    let movie = app.data_dir.join("Movie.mkv");
    make_testsrc_mkv(&movie, 2).await.unwrap();
    let original_size = std::fs::metadata(&movie).unwrap().len();

    // Seed a flow.
    let yaml = r#"
name: e2e
triggers: [{ radarr: [downloaded] }]
steps:
  - id: probe
    use: probe
  - id: enc
    use: transcode
    with: { codec: x264, crf: 30, preset: ultrafast }
  - id: out
    use: output
    with: { mode: replace }
"#;
    let flow = parse_flow(yaml).unwrap();
    db::flows::insert(&app.pool, "e2e", yaml, &flow)
        .await
        .unwrap();

    // Insert a radarr source so the auth lookup succeeds.
    db::sources::insert(&app.pool, "radarr", "main", &json_macro!({}), "test-token")
        .await
        .unwrap();

    // POST a Radarr-shaped payload.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/webhook/radarr", app.url))
        .bearer_auth("test-token")
        .json(&json!({
            "eventType": "Download",
            "movieFile": { "path": movie.to_string_lossy() }
        }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "got {}", resp.status());

    // Poll the DB until the job reaches a terminal status.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT status FROM jobs ORDER BY id DESC LIMIT 1")
                .fetch_optional(&app.pool)
                .await
                .unwrap();
        if let Some((status,)) = row {
            if status == "completed" {
                break;
            }
            if status == "failed" {
                panic!("job failed");
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("job did not complete in time");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // File still exists, with a likely smaller size.
    let new_size = std::fs::metadata(&movie).unwrap().len();
    assert!(new_size > 0);
    let _ = original_size;
}
