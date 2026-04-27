//! Regression test: DELETE /api/sources/:id must succeed even when
//! historical jobs reference the source via FK. Pre-fix, the FK on
//! jobs(source_id) → sources(id) blocked the DELETE and the handler
//! silently returned 500.

mod common;

use serde_json::json;

#[tokio::test]
async fn delete_source_succeeds_with_referencing_jobs_and_orphans_them() {
    let app = common::boot().await;
    let client = reqwest::Client::new();

    // Insert a manual source via the API.
    let create: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "generic",
            "name": "manual-test",
            "config": {},
            "secret_token": "tok"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create["id"].as_i64().unwrap();

    // Seed a flow + a job referencing the source. Skip the API: we
    // just need a row in `jobs` with `source_id = id` to reproduce the
    // FK constraint.
    sqlx::query(
        "INSERT INTO flows (name, enabled, yaml_source, parsed_json, version, updated_at) \
         VALUES ('test-flow', 1, 'name: test-flow\n', '{}', 1, 0)",
    )
    .execute(&app.pool)
    .await
    .unwrap();
    let flow_id: i64 = sqlx::query_scalar("SELECT id FROM flows WHERE name = 'test-flow'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO jobs (flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, source_id, created_at) \
         VALUES (?, 1, 'generic', '/x.mkv', '{}', 'completed', ?, 0)",
    )
    .bind(flow_id)
    .bind(id)
    .execute(&app.pool)
    .await
    .unwrap();

    // DELETE the source. Pre-fix this returned 500 because the FK
    // blocked the DELETE.
    let resp = client
        .delete(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        204,
        "expected 204 NO_CONTENT, got {}",
        resp.status()
    );

    // Source row is gone.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sources WHERE id = ?")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "source row was not deleted");

    // Job row is preserved with source_id NULL.
    let job_source_id: Option<i64> =
        sqlx::query_scalar("SELECT source_id FROM jobs WHERE flow_id = ? LIMIT 1")
            .bind(flow_id)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert!(
        job_source_id.is_none(),
        "job's source_id should be NULL after source delete; got {job_source_id:?}"
    );
}
