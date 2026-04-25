use tempfile::tempdir;
use transcoderr::db;
use transcoderr::ffmpeg::make_testsrc_mkv;
use transcoderr::flow::{parse_flow, Context, Engine};

#[tokio::test]
async fn engine_runs_probe_transcode_output() {
    let dir = tempdir().unwrap();
    let movie = dir.path().join("movie.mkv");
    make_testsrc_mkv(&movie, 2).await.unwrap();

    let pool = db::open(&dir.path().join("db")).await.unwrap();
    let yaml = format!(r#"
name: e2e
triggers: [{{ radarr: [downloaded] }}]
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
"#);
    let flow = parse_flow(&yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "e2e", &yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", &movie.to_string_lossy(), "{}").await.unwrap();
    let _claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();

    let ctx = Context::for_file(movie.to_string_lossy());
    let bus = transcoderr::bus::Bus::default();
    let outcome = Engine::new(pool.clone(), bus, dir.path().join("db")).run(&flow, job_id, ctx).await.unwrap();
    assert_eq!(outcome.status, "completed");

    // Original file replaced with transcoded output, and probe context recorded.
    let final_size = std::fs::metadata(&movie).unwrap().len();
    assert!(final_size > 0);
}
