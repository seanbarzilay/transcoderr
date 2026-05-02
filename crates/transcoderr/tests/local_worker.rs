//! Integration tests for the local-worker abstraction:
//! - boot populates the seeded `local` row
//! - heartbeat advances `last_seen_at`
//! - PATCH /api/workers/1 disabled stops claiming
//! - PATCH back to enabled resumes claiming

mod common;

use common::boot;
use serde_json::json;
use transcoderr::db;
use transcoderr::flow::parse_flow;
use transcoderr::worker::local::LOCAL_WORKER_ID;

/// Insert a flow + a pending job using the same shape as
/// `tests/concurrent_claim.rs` and `tests/crash_recovery.rs`. The flow
/// only needs to parse — the tests below only assert claim semantics
/// (whether the row leaves `pending`), not whether the engine completes.
async fn submit_simple_flow_job(app: &common::TestApp) -> i64 {
    let yaml = "name: t\ntriggers: [{ radarr: [downloaded] }]\nsteps:\n  - use: probe\n";
    let flow = parse_flow(yaml).unwrap();
    let flow_id = db::flows::insert(&app.pool, "t", yaml, &flow).await.unwrap();
    db::jobs::insert(&app.pool, flow_id, 1, "radarr", "/tmp/x.mkv", "{}")
        .await
        .unwrap()
}

async fn job_status(pool: &sqlx::SqlitePool, id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT status FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

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

#[tokio::test]
async fn disabled_local_worker_drains_and_stops_claiming() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Disable the local worker.
    let resp = client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "PATCH must succeed");

    // The pool's run_loop is already running. It checks `is_enabled`
    // before each `tick()`, but a tick already in flight can still call
    // `claim_next` after our PATCH lands. Give the loop one full
    // 500ms-gate cycle to observe the disabled flag before we insert
    // the job, otherwise the pool can race and claim before noticing.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    // Submit a job. Pool is gated; it should stay pending.
    let job_id = submit_simple_flow_job(&app).await;

    // Wait long enough for the pool's 500ms gate to have re-checked
    // multiple times.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let status = job_status(&app.pool, job_id).await;
    assert_eq!(
        status, "pending",
        "disabled local worker must not claim jobs (got {status})"
    );
}

#[tokio::test]
async fn re_enabling_resumes_dispatch() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Disable, submit, re-enable.
    client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();

    // Same race window as test 3: let the pool's run_loop observe the
    // disabled flag (one full 500ms gate cycle) before we make a job
    // visible. Otherwise an in-flight `tick()` can claim and burn our
    // pending job before the gate kicks in.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;

    let job_id = submit_simple_flow_job(&app).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(job_status(&app.pool, job_id).await, "pending");

    client
        .patch(format!("{}/api/workers/{LOCAL_WORKER_ID}", app.url))
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();

    // Poll for up to 5s for the job to leave 'pending'.
    let mut left_pending = false;
    for _ in 0..50 {
        let s = job_status(&app.pool, job_id).await;
        if s != "pending" {
            left_pending = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(left_pending, "re-enabling must let the pool claim");
}
