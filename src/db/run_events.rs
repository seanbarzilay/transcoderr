use crate::db::now_unix;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn append(
    pool: &SqlitePool,
    job_id: i64,
    step_id: Option<&str>,
    kind: &str,
    payload: Option<&Value>,
) -> anyhow::Result<()> {
    let payload_json = payload.map(|v| serde_json::to_string(v)).transpose()?;
    sqlx::query("INSERT INTO run_events (job_id, ts, step_id, kind, payload_json) VALUES (?, ?, ?, ?, ?)")
        .bind(job_id).bind(now_unix()).bind(step_id).bind(kind).bind(payload_json)
        .execute(pool).await?;
    Ok(())
}
