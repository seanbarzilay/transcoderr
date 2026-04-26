mod common;
use common::boot;
use futures::StreamExt;
use std::time::Duration;
use transcoderr::{db, ffmpeg::make_testsrc_mkv, flow::parse_flow};

#[tokio::test]
async fn sse_stream_emits_run_events() {
    let app = boot().await;

    // Open SSE in the background.
    let url = format!("{}/api/stream", app.url);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.unwrap();
    assert!(resp.status().is_success());
    let mut byte_stream = resp.bytes_stream();

    // Trigger a job.
    let movie = app.data_dir.join("Movie.mkv");
    make_testsrc_mkv(&movie, 1).await.unwrap();
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
    db::flows::insert(&app.pool, "e2e", yaml, &flow).await.unwrap();
    db::sources::insert(&app.pool, "radarr", "main", &serde_json::json!({}), "tok").await.unwrap();
    let _ = client.post(format!("{}/webhook/radarr", app.url))
        .bearer_auth("tok")
        .json(&serde_json::json!({"eventType":"Download","movieFile":{"path":movie.to_string_lossy()}}))
        .send().await.unwrap();

    // Read events for ~10s.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut got_run_event = false;
    let mut got_job_state_completed = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), byte_stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                let s = String::from_utf8_lossy(&chunk);
                if s.contains("\"topic\":\"RunEvent\"") { got_run_event = true; }
                if s.contains("\"topic\":\"JobState\"") && s.contains("completed") { got_job_state_completed = true; }
                if got_run_event && got_job_state_completed { break; }
            }
            _ => {}
        }
    }
    assert!(got_run_event, "no RunEvent observed");
    assert!(got_job_state_completed, "no completed JobState observed");
}
