use tempfile::tempdir;
use transcoderr::{db, flow::{parse_flow, Context, Engine}};

#[tokio::test]
async fn shell_step_timeout_fails_quickly() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    transcoderr::steps::registry::init(pool.clone(), transcoderr::hw::semaphores::DeviceRegistry::empty(), std::sync::Arc::new(transcoderr::ffmpeg_caps::FfmpegCaps::default()), vec![]).await;

    let yaml = r#"
name: t
triggers: [{ radarr: [downloaded] }]
steps:
  - use: shell
    with:
      cmd: "sleep 5"
      timeout: 1
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "t", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/x", "{}").await.unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    let start = std::time::Instant::now();
    let bus = transcoderr::bus::Bus::default();
    let outcome = Engine::new(pool.clone(), bus, dir.path().to_path_buf()).run(&flow, job_id, Context::for_file("/x")).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(outcome.status, "failed");
    assert!(elapsed.as_secs() < 3, "timeout should fire within ~1-2s, took {:?}", elapsed);

    // Verify a 'failed' run_event with timeout in the payload exists.
    let evt: Option<(String,)> = sqlx::query_as(
        "SELECT payload_json FROM run_events WHERE job_id = ? AND kind = 'failed' ORDER BY id DESC LIMIT 1"
    ).bind(job_id).fetch_optional(&pool).await.unwrap();
    let payload = evt.unwrap().0;
    assert!(payload.contains("timeout"), "expected timeout in payload: {payload}");
}
