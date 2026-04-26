use crate::bus::Bus;
use crate::cancellation::JobCancellations;
use crate::db;
use crate::flow::{Context, Engine, Flow};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::time::Duration;

pub struct Worker {
    pool: SqlitePool,
    bus: Bus,
    data_dir: PathBuf,
    cancellations: JobCancellations,
}

impl Worker {
    pub fn new(
        pool: SqlitePool,
        bus: Bus,
        data_dir: PathBuf,
        cancellations: JobCancellations,
    ) -> Self {
        Self { pool, bus, data_dir, cancellations }
    }

    /// On startup: reset stale 'running' rows back to 'pending'.
    pub async fn recover_on_boot(&self) -> anyhow::Result<u64> {
        db::jobs::reset_running_to_pending(&self.pool).await
    }

    /// One loop iteration: claim and run one job. Returns true if a job was processed.
    pub async fn tick(&self) -> anyhow::Result<bool> {
        self.broadcast_queue_snapshot().await;

        let Some(job) = db::jobs::claim_next(&self.pool).await? else { return Ok(false); };

        // Job just transitioned pending → running; refresh the snapshot so the
        // Dashboard's "Running" tile reflects the change immediately.
        self.broadcast_queue_snapshot().await;
        // Load flow.
        let flow_row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT name, yaml_source, parsed_json FROM flows WHERE id = ?"
        ).bind(job.flow_id).fetch_optional(&self.pool).await?;
        let (flow_name, _, parsed_json) = flow_row.ok_or_else(|| anyhow::anyhow!("flow {} missing", job.flow_id))?;
        let flow: Flow = serde_json::from_str(&parsed_json)?;

        // Mint a cancellation token for this job and propagate it through the
        // engine into Context. The API's cancel handler triggers it; the engine
        // and ffmpeg helpers race on it and bail.
        let cancel_token = self.cancellations.register(job.id);
        let mut ctx = Context::for_file(&job.file_path);
        ctx.cancel = Some(cancel_token.clone());

        let job_start = std::time::Instant::now();
        let outcome = Engine::new(self.pool.clone(), self.bus.clone(), self.data_dir.clone())
            .run(&flow, job.id, ctx)
            .await?;
        let elapsed_secs = job_start.elapsed().as_secs_f64();

        // If the user cancelled mid-step the engine returns a 'failed' outcome
        // (the step's run_with_live_events bailed with "cancelled"). Distinguish
        // cancellation from genuine failure when persisting the final status.
        let final_status = if cancel_token.is_cancelled() && outcome.status != "completed" {
            "cancelled".to_string()
        } else {
            outcome.status.clone()
        };

        self.cancellations.unregister(job.id);
        crate::metrics::record_job_finished(&flow_name, &final_status, elapsed_secs);
        db::jobs::set_status_with_bus(
            &self.pool,
            &self.bus,
            job.id,
            &final_status,
            outcome.label.as_deref(),
        )
        .await?;

        // Job left the running set; refresh the snapshot.
        self.broadcast_queue_snapshot().await;

        Ok(true)
    }

    /// Count pending + running jobs and broadcast onto the bus, plus update the
    /// Prometheus queue gauge. Called whenever the queue state changes (worker
    /// tick start, claim, finish) so the Dashboard tiles track in real time.
    async fn broadcast_queue_snapshot(&self) {
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE status = 'pending'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE status = 'running'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        crate::metrics::set_queue_depth(pending);
        let _ = self.bus.tx.send(crate::bus::Event::Queue { pending, running });
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
