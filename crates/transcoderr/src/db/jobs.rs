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
    flow_id: i64,
    flow_version: i64,
    source_kind: &str,
    file_path: &str,
    payload: &str,
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
/// claim_next calls when runs.max_concurrent > 1, hitting SQLITE_BUSY).
///
/// A pending job is skipped while another job for the same `file_path` is
/// already running. Every webhook enqueues one job per matching flow for
/// the same file, and `runs.max_concurrent` defaults to 2, so without this
/// two runs would process one media file simultaneously: both stage
/// intermediates next to it and both `output: replace` rename over the
/// original. Such jobs are not dropped, just deferred — the next tick
/// picks them up once the file is free. Boot recovery flips abandoned
/// 'running' rows back to 'pending', so a crash cannot block a file
/// permanently.
pub async fn claim_next(pool: &SqlitePool) -> anyhow::Result<Option<JobRow>> {
    let row: Option<JobRow> = sqlx::query_as(
        "UPDATE jobs SET status = 'running', started_at = ?, attempt = attempt + 1 \
         WHERE id = ( \
            SELECT id FROM jobs WHERE status = 'pending' \
              AND file_path NOT IN (SELECT file_path FROM jobs WHERE status = 'running') \
            ORDER BY priority DESC, created_at ASC LIMIT 1 \
         ) AND status = 'pending' \
         RETURNING id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, current_step, attempt"
    )
    .bind(now_unix())
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn set_status(
    pool: &SqlitePool,
    id: i64,
    status: &str,
    label: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET status = ?, status_label = ?, finished_at = ? WHERE id = ?")
        .bind(status)
        .bind(label)
        .bind(now_unix())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_status_with_bus(
    pool: &SqlitePool,
    bus: &crate::bus::Bus,
    id: i64,
    status: &str,
    label: Option<&str>,
) -> anyhow::Result<()> {
    set_status(pool, id, status, label).await?;
    bus.send(crate::bus::Event::JobState {
        id,
        status: status.to_string(),
        label: label.map(|s| s.to_string()),
    });
    Ok(())
}

pub async fn set_current_step(pool: &SqlitePool, id: i64, step_index: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET current_step = ? WHERE id = ?")
        .bind(step_index)
        .bind(id)
        .execute(pool)
        .await?;
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
    let r = sqlx::query(
        "UPDATE jobs SET status = 'pending', started_at = NULL WHERE status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// Stamp the job's `worker_id`. Called by `Engine::run_nodes` at the
/// first dispatch decision (local or remote) so the run row reflects
/// its primary executor for backwards-compatible UI.
pub async fn set_worker_id(pool: &SqlitePool, job_id: i64, worker_id: i64) -> anyhow::Result<()> {
    sqlx::query("UPDATE jobs SET worker_id = ? WHERE id = ?")
        .bind(worker_id)
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod claim_tests {
    use super::*;
    use tempfile::tempdir;

    async fn pool_with_flow() -> (SqlitePool, i64, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        // jobs.flow_id has an FK to flows and the foreign_keys pragma is on.
        let flow_id: i64 = sqlx::query_scalar(
            "INSERT INTO flows (name, yaml_source, parsed_json, enabled, version, updated_at) \
             VALUES ('t', '', '{}', 1, 1, 0) RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        (pool, flow_id, dir)
    }

    #[tokio::test]
    async fn claim_skips_a_file_already_running() {
        let (pool, flow_id, _dir) = pool_with_flow().await;
        // Two jobs for the same file — what one webhook produces when two
        // flows match it.
        let first = insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();
        let second = insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();

        let claimed = claim_next(&pool).await.unwrap().expect("first claim");
        assert_eq!(claimed.id, first);

        assert!(
            claim_next(&pool).await.unwrap().is_none(),
            "job {second} must not be claimed while {first} is running on the same file"
        );
    }

    #[tokio::test]
    async fn claim_takes_a_different_file_meanwhile() {
        // Deferral must be per-file, not a global stall.
        let (pool, flow_id, _dir) = pool_with_flow().await;
        let busy = insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();
        insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();
        let other = insert(&pool, flow_id, 1, "radarr", "/m/Arrival.mkv", "{}")
            .await
            .unwrap();

        assert_eq!(claim_next(&pool).await.unwrap().unwrap().id, busy);
        assert_eq!(
            claim_next(&pool).await.unwrap().expect("other file").id,
            other
        );
    }

    #[tokio::test]
    async fn deferred_job_is_claimable_once_the_file_is_free() {
        // Deferred, not dropped.
        let (pool, flow_id, _dir) = pool_with_flow().await;
        let first = insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();
        let second = insert(&pool, flow_id, 1, "radarr", "/m/Dune.mkv", "{}")
            .await
            .unwrap();

        assert_eq!(claim_next(&pool).await.unwrap().unwrap().id, first);
        assert!(claim_next(&pool).await.unwrap().is_none());

        set_status(&pool, first, "completed", None).await.unwrap();

        assert_eq!(
            claim_next(&pool)
                .await
                .unwrap()
                .expect("second job once the file is free")
                .id,
            second
        );
    }
}
