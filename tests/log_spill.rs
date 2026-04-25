use serde_json::json;
use tempfile::tempdir;
use transcoderr::db;

#[tokio::test]
async fn large_payload_spills_to_file() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    sqlx::query("INSERT INTO flows (id, name, enabled, yaml_source, parsed_json, version, updated_at) VALUES (1, 'x', 1, '', '{}', 1, 0)")
        .execute(&pool).await.unwrap();
    let job_id = db::jobs::insert(&pool, 1, 1, "radarr", "/x", "{}").await.unwrap();
    let big = json!({ "blob": "a".repeat(100 * 1024) });
    db::run_events::append_with_spill(&pool, dir.path(), job_id, Some("step1"), "log", Some(&big)).await.unwrap();
    let row: (Option<String>, Option<String>) = sqlx::query_as("SELECT payload_json, payload_path FROM run_events ORDER BY id DESC LIMIT 1")
        .fetch_one(&pool).await.unwrap();
    assert!(row.0.is_none(), "payload_json should be empty for spill");
    let path = row.1.expect("payload_path set");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.len() >= 100 * 1024);
}
