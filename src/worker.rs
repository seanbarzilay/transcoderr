use crate::bus::Bus;
use crate::db;
use crate::flow::{Context, Engine, Flow};
use sqlx::SqlitePool;
use std::time::Duration;

pub struct Worker {
    pool: SqlitePool,
    bus: Bus,
}

impl Worker {
    pub fn new(pool: SqlitePool, bus: Bus) -> Self { Self { pool, bus } }

    /// On startup: reset stale 'running' rows back to 'pending'.
    pub async fn recover_on_boot(&self) -> anyhow::Result<u64> {
        db::jobs::reset_running_to_pending(&self.pool).await
    }

    /// One loop iteration: claim and run one job. Returns true if a job was processed.
    pub async fn tick(&self) -> anyhow::Result<bool> {
        // Update queue depth metric.
        let depth: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status='pending'")
            .fetch_one(&self.pool).await.unwrap_or(0);
        crate::metrics::set_queue_depth(depth);

        let Some(job) = db::jobs::claim_next(&self.pool).await? else { return Ok(false); };
        // Load flow.
        let flow_row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT name, yaml_source, parsed_json FROM flows WHERE id = ?"
        ).bind(job.flow_id).fetch_optional(&self.pool).await?;
        let (flow_name, _, parsed_json) = flow_row.ok_or_else(|| anyhow::anyhow!("flow {} missing", job.flow_id))?;
        let flow: Flow = serde_json::from_str(&parsed_json)?;

        let ctx = Context::for_file(&job.file_path);
        let job_start = std::time::Instant::now();
        let outcome = Engine::new(self.pool.clone(), self.bus.clone()).run(&flow, job.id, ctx).await?;
        let elapsed_secs = job_start.elapsed().as_secs_f64();
        crate::metrics::record_job_finished(&flow_name, &outcome.status, elapsed_secs);
        db::jobs::set_status_with_bus(&self.pool, &self.bus, job.id, &outcome.status, outcome.label.as_deref()).await?;
        Ok(true)
    }

    pub async fn run_loop(&self, shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut shutdown = shutdown;
        loop {
            if *shutdown.borrow() { return; }
            match self.tick().await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "worker tick failed");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }
}
