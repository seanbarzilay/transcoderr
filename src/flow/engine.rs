use crate::db;
use crate::flow::{Context, Flow};
use crate::steps::{dispatch, StepProgress};
use serde_json::json;
use sqlx::SqlitePool;

pub struct Engine {
    pool: SqlitePool,
}

#[derive(Debug)]
pub struct Outcome {
    pub status: String,
    pub label: Option<String>,
}

impl Engine {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }

    pub async fn run(&self, flow: &Flow, job_id: i64, mut ctx: Context) -> anyhow::Result<Outcome> {
        // Resume from checkpoint if any.
        let resume_index = match db::checkpoints::get(&self.pool, job_id).await? {
            Some((idx, snap)) => {
                ctx = Context::from_snapshot(&snap)?;
                idx + 1
            }
            None => 0,
        };

        for (idx, step) in flow.steps.iter().enumerate().skip(resume_index as usize) {
            let step_id = step.id.clone().unwrap_or_else(|| format!("step{idx}"));
            db::jobs::set_current_step(&self.pool, job_id, idx as i64).await?;
            db::run_events::append(&self.pool, job_id, Some(&step_id), "started",
                Some(&json!({ "use": step.use_ }))).await?;

            let runner = dispatch(&step.use_)
                .ok_or_else(|| anyhow::anyhow!("unknown step `use:` {}", step.use_))?;

            let pool = self.pool.clone();
            let step_id_for_cb = step_id.clone();
            let mut cb = move |ev: StepProgress| {
                let pool = pool.clone();
                let step_id = step_id_for_cb.clone();
                tokio::spawn(async move {
                    let (kind, payload) = match ev {
                        StepProgress::Pct(p) => ("progress", json!({ "pct": p })),
                        StepProgress::Log(l) => ("log", json!({ "msg": l })),
                    };
                    let _ = db::run_events::append(&pool, job_id, Some(&step_id), kind, Some(&payload)).await;
                });
            };

            match runner.execute(&step.with, &mut ctx, &mut cb).await {
                Ok(()) => {
                    db::run_events::append(&self.pool, job_id, Some(&step_id), "completed", None).await?;
                    db::checkpoints::upsert(&self.pool, job_id, idx as i64, &ctx.to_snapshot()).await?;
                }
                Err(e) => {
                    db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
                        Some(&json!({ "error": e.to_string() }))).await?;
                    return Ok(Outcome { status: "failed".into(), label: None });
                }
            }
        }

        Ok(Outcome { status: "completed".into(), label: None })
    }
}
