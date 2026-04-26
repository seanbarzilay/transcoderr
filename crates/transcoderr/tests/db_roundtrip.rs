use tempfile::tempdir;
use transcoderr::db;
use transcoderr::flow::parse_flow;

#[tokio::test]
async fn flow_and_job_roundtrip() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    let yaml = r#"
name: t
triggers: [{ radarr: [downloaded] }]
steps:
  - use: probe
"#;
    let flow = parse_flow(yaml).unwrap();
    let id = db::flows::insert(&pool, "t", yaml, &flow).await.unwrap();
    assert!(id > 0);
    let job_id = db::jobs::insert(&pool, id, 1, "radarr", "/tmp/x.mkv", "{}").await.unwrap();
    let claimed = db::jobs::claim_next(&pool).await.unwrap().unwrap();
    assert_eq!(claimed.id, job_id);
    db::jobs::set_status(&pool, job_id, "completed", None).await.unwrap();
    let none = db::jobs::claim_next(&pool).await.unwrap();
    assert!(none.is_none());
}
