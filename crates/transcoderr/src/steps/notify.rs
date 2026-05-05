use super::{Step, StepProgress};
use crate::db;
use crate::flow::{expr, Context};
use crate::notifiers;
use async_trait::async_trait;
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

pub struct NotifyStep {
    pub pool: SqlitePool,
}

#[async_trait]
impl Step for NotifyStep {
    fn name(&self) -> &'static str {
        "notify"
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::notify_schema())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let channel = with
            .get("channel")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("notify: missing `channel`"))?;
        let template = with.get("template").and_then(|v| v.as_str()).unwrap_or("");
        let message = expr::eval_string_template(template, ctx)?;
        on_progress(StepProgress::Log(format!("notify {channel}: {message}")));
        let row = db::notifiers::get_by_name(&self.pool, channel)
            .await?
            .ok_or_else(|| anyhow::anyhow!("notify: notifier {channel:?} not configured"))?;
        let cfg: Value = serde_json::from_str(&row.config_json)?;
        let notifier = notifiers::build(&row.kind, &cfg)?;
        notifier
            .send(&message, &json!({"file": ctx.file.path}))
            .await?;
        Ok(())
    }
}
