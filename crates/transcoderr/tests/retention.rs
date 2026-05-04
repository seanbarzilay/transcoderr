use tempfile::tempdir;
use transcoderr::{db, retention};

#[tokio::test]
async fn run_once_prunes_old_completed_jobs() {
    let dir = tempdir().unwrap();
    let pool = db::open(dir.path()).await.unwrap();
    db::settings::set(&pool, "retention.events_days", "1")
        .await
        .unwrap();
    db::settings::set(&pool, "retention.jobs_days", "1")
        .await
        .unwrap();
    sqlx::query("INSERT INTO flows (id, name, enabled, yaml_source, parsed_json, version, updated_at) VALUES (1, 'x', 1, '', '{}', 1, 0)")
        .execute(&pool).await.unwrap();
    let two_days = chrono::Utc::now().timestamp() - 2 * 86_400;
    sqlx::query("INSERT INTO jobs (id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at, finished_at) VALUES (1, 1, 1, 'radarr', '/x', '{}', 'completed', 0, 0, ?, ?)")
        .bind(two_days).bind(two_days).execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO run_events (job_id, ts, kind, payload_json) VALUES (1, ?, 'completed', '{}')",
    )
    .bind(two_days)
    .execute(&pool)
    .await
    .unwrap();

    retention::run_once(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let m: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM run_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(m, 0);
}
