use crate::db;
use sqlx::SqlitePool;
use std::time::Duration;

pub async fn run_periodic(pool: SqlitePool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        if let Err(e) = run_once(&pool).await {
            tracing::warn!(error = %e, "retention pass failed");
        }
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(Duration::from_secs(60 * 60 * 24)) => {}
        }
    }
}

pub async fn run_once(pool: &SqlitePool) -> anyhow::Result<()> {
    let events_days: i64 = db::settings::get(pool, "retention.events_days")
        .await?
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    let jobs_days: i64 = db::settings::get(pool, "retention.jobs_days")
        .await?
        .and_then(|s| s.parse().ok())
        .unwrap_or(90);

    let now = chrono::Utc::now().timestamp();
    let event_cutoff = now - events_days * 86_400;
    let job_cutoff = now - jobs_days * 86_400;

    sqlx::query("DELETE FROM run_events WHERE job_id IN (SELECT id FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?)")
        .bind(event_cutoff).execute(pool).await?;
    sqlx::query("DELETE FROM checkpoints WHERE job_id IN (SELECT id FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?)")
        .bind(event_cutoff).execute(pool).await?;
    sqlx::query("DELETE FROM jobs WHERE finished_at IS NOT NULL AND finished_at < ?")
        .bind(job_cutoff)
        .execute(pool)
        .await?;
    sqlx::query("VACUUM").execute(pool).await?;
    Ok(())
}
