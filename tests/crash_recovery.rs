use tempfile::tempdir;
use transcoderr::{db, ffmpeg::make_testsrc_mkv, flow::{parse_flow, Context, Engine}, worker::Worker};

#[tokio::test]
async fn checkpoint_resume_after_simulated_crash() {
    let dir = tempdir().unwrap();
    let movie = dir.path().join("m.mkv");
    make_testsrc_mkv(&movie, 1).await.unwrap();

    let pool = db::open(&dir.path().join("db")).await.unwrap();
    let yaml = r#"
name: r
triggers: [{ radarr: [downloaded] }]
steps:
  - id: probe
    use: probe
  - id: enc
    use: transcode
    with:
      codec: x264
      crf: 30
      preset: ultrafast
  - id: out
    use: output
    with:
      mode: replace
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "r", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", &movie.to_string_lossy(), "{}").await.unwrap();
    let _claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    // Simulate: probe ran, checkpoint saved at index 0, then "crash"
    let mut ctx = Context::for_file(movie.to_string_lossy());
    transcoderr::steps::dispatch("probe").unwrap()
        .execute(&Default::default(), &mut ctx, &mut |_| {})
        .await.unwrap();
    db::checkpoints::upsert(&pool, job_id, 0, &ctx.to_snapshot()).await.unwrap();
    // Process "crashes"; row left in 'running'

    // Boot recovery
    let bus = transcoderr::bus::Bus::default();
    let w = Worker::new(pool.clone(), bus.clone(), dir.path().join("db"));
    let reset = w.recover_on_boot().await.unwrap();
    assert_eq!(reset, 1, "should reset one running job");

    // Run engine — should resume from checkpoint, skipping probe.
    let outcome = Engine::new(pool.clone(), bus, dir.path().join("db")).run(&flow, job_id, Context::for_file(movie.to_string_lossy())).await.unwrap();
    assert_eq!(outcome.status, "completed");

    // The probe-skipped path leaves probe-step events absent from this run leg —
    // verify we DIDN'T re-run probe by inspecting that no NEW probe event fired
    // after the checkpoint was set.
    let evts: Vec<(String,)> = sqlx::query_as("SELECT kind FROM run_events WHERE job_id = ? AND step_id = 'probe'")
        .bind(job_id).fetch_all(&pool).await.unwrap();
    assert_eq!(evts.len(), 0, "probe should have been skipped due to checkpoint resume");
}
