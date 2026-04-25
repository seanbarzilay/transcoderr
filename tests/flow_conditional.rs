use tempfile::tempdir;
use transcoderr::{db, flow::{parse_flow, Context, Engine}};

#[tokio::test]
async fn conditional_then_branch_runs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: c
triggers: [{ radarr: [downloaded] }]
steps:
  - id: gate
    if: file.path == "/m/x.mkv"
    then:
      - return: matched
    else:
      - return: missed
"#;
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&pool, "c", yaml, &flow).await.unwrap();
    let job_id = db::jobs::insert(&pool, flow_id, 1, "radarr", "/m/x.mkv", "{}").await.unwrap();
    let _ = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    let bus = transcoderr::bus::Bus::default();
    let outcome = Engine::new(pool.clone(), bus).run(&flow, job_id, Context::for_file("/m/x.mkv")).await.unwrap();
    assert_eq!(outcome.status, "skipped");
    assert_eq!(outcome.label.as_deref(), Some("matched"));
}
