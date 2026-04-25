use crate::db;
use crate::flow::{expr, Context, Flow, Node};
use crate::steps::{registry::resolve, StepProgress};
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
        // Resume.
        let resume = match db::checkpoints::get(&self.pool, job_id).await? {
            Some((idx, snap)) => {
                ctx = Context::from_snapshot(&snap)?;
                Some(idx as u32 + 1)
            }
            None => None,
        };

        let mut counter = 0u32;
        match self.run_nodes(&flow.steps, job_id, &mut ctx, &mut counter, resume).await {
            Ok(NodeOutcome::Continue) => Ok(Outcome { status: "completed".into(), label: None }),
            Ok(NodeOutcome::Return(label)) => Ok(Outcome { status: "skipped".into(), label: Some(label) }),
            Err(e) => {
                db::run_events::append(&self.pool, job_id, None, "failed",
                    Some(&json!({ "error": e.to_string() }))).await?;
                if let Some(of) = &flow.on_failure {
                    // Run failure handler with a small ctx extension.
                    let mut counter2 = u32::MAX / 2; // distinct space, never checkpointed
                    let _ = self.run_nodes(of, job_id, &mut ctx, &mut counter2, None).await;
                }
                Ok(Outcome { status: "failed".into(), label: None })
            }
        }
    }

    fn run_nodes<'a>(
        &'a self, nodes: &'a [Node], job_id: i64, ctx: &'a mut Context,
        counter: &'a mut u32, resume_at: Option<u32>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<NodeOutcome>> + Send + 'a>> {
        Box::pin(async move {
            for n in nodes {
                let my_index = *counter;
                *counter += 1;
                if let Some(skip_below) = resume_at {
                    if my_index < skip_below { continue; }
                }
                match n {
                    Node::Step { id, use_, with, retry } => {
                        let step_id = id.clone().unwrap_or_else(|| format!("{use_}_{my_index}"));
                        let max_attempts = retry.as_ref().map(|r| r.max + 1).unwrap_or(1);
                        let mut last_err: Option<anyhow::Error> = None;
                        for attempt in 1..=max_attempts {
                            db::run_events::append(&self.pool, job_id, Some(&step_id), "started",
                                Some(&json!({ "use": use_, "attempt": attempt }))).await?;
                            let runner = resolve(use_).await
                                .ok_or_else(|| anyhow::anyhow!("unknown step `use:` {}", use_))?;
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
                            let timeout_secs = with.get("timeout")
                                .and_then(|v| v.as_u64())
                                .unwrap_or_else(|| match use_.as_str() {
                                    "transcode" => 86_400,
                                    "probe" | "verify.playable" => 60,
                                    _ => 600,
                                });
                            let exec_fut = runner.execute(with, ctx, &mut cb);
                            let result = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), exec_fut).await;
                            match result {
                                Ok(Ok(())) => {
                                    db::run_events::append(&self.pool, job_id, Some(&step_id), "completed", None).await?;
                                    db::checkpoints::upsert(&self.pool, job_id, my_index as i64, &ctx.to_snapshot()).await?;
                                    last_err = None;
                                    break;
                                }
                                Ok(Err(e)) => {
                                    db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
                                        Some(&json!({ "error": e.to_string(), "attempt": attempt }))).await?;
                                    let should_retry = retry.as_ref().and_then(|r| r.on.as_deref())
                                        .map(|on_expr| expr::eval_bool(on_expr, ctx).unwrap_or(true))
                                        .unwrap_or(true);
                                    if !should_retry || attempt == max_attempts {
                                        last_err = Some(e);
                                        break;
                                    }
                                    last_err = Some(e);
                                }
                                Err(_) => {
                                    db::run_events::append(&self.pool, job_id, Some(&step_id), "failed",
                                        Some(&json!({ "error": "timeout", "after_seconds": timeout_secs, "attempt": attempt }))).await?;
                                    last_err = Some(anyhow::anyhow!("timeout after {timeout_secs}s"));
                                    break;
                                }
                            }
                        }
                        if let Some(e) = last_err { return Err(e); }
                    }
                    Node::Conditional { id, if_, then_, else_ } => {
                        let step_id = id.clone().unwrap_or_else(|| format!("if_{my_index}"));
                        let v = expr::eval_bool(if_, ctx)?;
                        db::run_events::append(&self.pool, job_id, Some(&step_id), "condition_evaluated",
                            Some(&json!({ "expr": if_, "result": v }))).await?;
                        let branch = if v { then_.as_slice() } else { else_.as_deref().unwrap_or(&[]) };
                        let outcome = self.run_nodes(branch, job_id, ctx, counter, resume_at).await?;
                        if let NodeOutcome::Return(_) = &outcome { return Ok(outcome); }
                    }
                    Node::Return { return_ } => {
                        db::run_events::append(&self.pool, job_id, None, "returned",
                            Some(&json!({ "label": return_ }))).await?;
                        return Ok(NodeOutcome::Return(return_.clone()));
                    }
                }
            }
            Ok(NodeOutcome::Continue)
        })
    }
}

#[derive(Debug)]
enum NodeOutcome {
    Continue,
    Return(String),
}
