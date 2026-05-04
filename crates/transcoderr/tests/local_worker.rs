//! Integration tests for the local-worker abstraction:
//! - boot populates the seeded `local` row
//! - heartbeat advances `last_seen_at`
//!
//! Disabling the local worker via the API is now blocked at the
//! handler layer (see `tests/workers_api.rs::patch_refuses_to_disable_local_worker`),
//! so the previous "PATCH disabled stops claiming" / "re-enable resumes
//! dispatch" tests have been retired — their premise no longer applies.

mod common;

use common::boot;
use transcoderr::worker::local::LOCAL_WORKER_ID;

#[tokio::test]
async fn local_row_populated_after_boot() {
    let app = boot().await;

    let row: (Option<String>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT hw_caps_json, plugin_manifest_json, last_seen_at
           FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    assert!(row.0.is_some(), "hw_caps_json must be populated");
    assert!(row.1.is_some(), "plugin_manifest_json must be populated");
    assert!(row.2.is_some(), "last_seen_at must be set");
}

#[tokio::test]
async fn heartbeat_advances_last_seen_when_idle() {
    let app = boot().await;

    let initial: i64 = sqlx::query_scalar(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    // Don't wait the real 30s tick. Force one explicit heartbeat
    // after a >1s pause so unix-second granularity advances.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    transcoderr::db::workers::record_heartbeat(&app.pool, LOCAL_WORKER_ID)
        .await
        .unwrap();

    let after: i64 = sqlx::query_scalar(
        "SELECT COALESCE(last_seen_at, 0) FROM workers WHERE id = ?",
    )
    .bind(LOCAL_WORKER_ID)
    .fetch_one(&app.pool)
    .await
    .unwrap();

    assert!(
        after > initial,
        "heartbeat must advance last_seen_at (was {initial}, now {after})"
    );
}
