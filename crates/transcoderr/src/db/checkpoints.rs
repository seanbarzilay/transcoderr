use crate::db::now_unix;
use sqlx::SqlitePool;

pub async fn upsert(pool: &SqlitePool, job_id: i64, step_index: i64, snapshot: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO checkpoints (job_id, step_index, context_snapshot_json, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT (job_id) DO UPDATE SET step_index = excluded.step_index, context_snapshot_json = excluded.context_snapshot_json, updated_at = excluded.updated_at"
    )
    .bind(job_id).bind(step_index).bind(snapshot).bind(now_unix())
    .execute(pool).await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, job_id: i64) -> anyhow::Result<Option<(i64, String)>> {
    Ok(sqlx::query_as("SELECT step_index, context_snapshot_json FROM checkpoints WHERE job_id = ?")
        .bind(job_id).fetch_optional(pool).await?)
}
