use tempfile::tempdir;
use transcoderr::{db, flow::{parse_flow, Context, Engine}};

#[tokio::test]
async fn on_failure_handler_runs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: f
triggers: [{ radarr: [downloaded] }]
steps:
  - use: probe       # will fail because file doesn't exist
on_failure:
  - use: shell
    with: { cmd: "echo handler ran" }
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "f", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/no/such/file.mkv", "{}").await.unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    let outcome = Engine::new(pool.clone()).run(&flow, job_id, Context::for_file("/no/such/file.mkv")).await.unwrap();
    assert_eq!(outcome.status, "failed");

    // shell step is added in Phase 2 Task 9. For now mark this test #[ignore]
    // and remove the ignore after Task 9.
}
