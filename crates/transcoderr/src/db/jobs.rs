use crate::db::now_unix;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobRow {
    pub id: i64,
    pub flow_id: i64,
    pub flow_version: i64,
    pub source_kind: String,
    pub file_path: String,
    pub trigger_payload_json: String,
    pub status: String,
    pub priority: i64,
    pub current_step: Option<i64>,
    pub attempt: i64,
}

pub async fn insert(
    pool: &SqlitePool,
    flow_id: i64, flow_version: i64,
    source_kind: &str, file_path: &str, payload: &str,
) -> anyhow::Result<i64> {
    let now = now_unix();
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO jobs (flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at) \
         VALUES (?, ?, ?, ?, ?, 'pending', 0, 0, ?) RETURNING id"
    )
    .bind(flow_id).bind(flow_version).bind(source_kind)
    .bind(file_path).bind(payload).bind(now)
    .fetch_one(pool).await?;
    Ok(id)
}

/// Atomically claim the next pending job — flips its status to running.
/// Returns None when no pending job exists OR when another worker beat
/// us in a race. Uses a single UPDATE...RETURNING so we don't need a
/// multi-statement transaction (which deadlocks under concurrent
/// claim_next calls when pool_size > 1, hitting SQLITE_BUSY).
pub async fn claim_next(pool: &SqlitePool) -> anyhow::Result<Option<JobRow>> {
    let row: Option<JobRow> = sqlx::query_as(
        "UPDATE jobs SET status = 'running', started_at = ?, attempt = attempt + 1 \
         WHERE id = ( \
            SELECT id FROM jobs WHERE status = 'pending' \
            ORDER BY priority DESC, created_at ASC LIMIT 1 \
         ) AND status = 'pending' \
         RETURNING id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, current_step, attempt"
    )
    .bind(now_unix())
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn set_status(pool: &SqlitePool, id: i64, status: &str, label: Option<&str>) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = ?, status_label = ?, finished_at = ? WHERE id = ?")
        .bind(status).bind(label).bind(now_unix()).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_status_with_bus(
    pool: &SqlitePool, bus: &crate::bus::Bus,
    id: i64, status: &str, label: Option<&str>,
) -> anyhow::Result<()> {
    set_status(pool, id, status, label).await?;
    bus.send(crate::bus::Event::JobState {
        id, status: status.to_string(), label: label.map(|s| s.to_string()),
    });
    Ok(())
}

pub async fn set_current_step(pool: &SqlitePool, id: i64, step_index: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET current_step = ? WHERE id = ?")
        .bind(step_index).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn insert_with_source(
    pool: &SqlitePool,
    flow_id: i64,
    flow_version: i64,
    source_id: i64,
    source_kind: &str,
    file_path: &str,
    payload: &str,
) -> anyhow::Result<i64> {
    let now = now_unix();
    Ok(sqlx::query_scalar::<_, i64>(
        "INSERT INTO jobs (flow_id, flow_version, source_id, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'pending', 0, 0, ?) RETURNING id"
    )
    .bind(flow_id)
    .bind(flow_version)
    .bind(source_id)
    .bind(source_kind)
    .bind(file_path)
    .bind(payload)
    .bind(now)
    .fetch_one(pool)
    .await?)
}

/// Reset 'running' rows to 'pending' on boot. Returns the number reset.
pub async fn reset_running_to_pending(pool: &SqlitePool) -> anyhow::Result<u64> {
    let r = sqlx::query("UPDATE jobs SET status = 'pending', started_at = NULL WHERE status = 'running'")
        .execute(pool).await?;
    Ok(r.rows_affected())
}

/// Stamp the job's `worker_id`. Called by `Engine::run_nodes` at the
/// first dispatch decision (local or remote) so the run row reflects
/// its primary executor for backwards-compatible UI.
pub async fn set_worker_id(
    pool: &SqlitePool,
    job_id: i64,
    worker_id: i64,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET worker_id = ? WHERE id = ?")
        .bind(worker_id)
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}
