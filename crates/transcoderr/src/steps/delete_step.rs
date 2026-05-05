use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct DeleteStep;

#[async_trait]
impl Step for DeleteStep {
    fn name(&self) -> &'static str {
        "delete"
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::empty_schema())
    }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let p = std::path::Path::new(&ctx.file.path);
        on_progress(StepProgress::Log(format!("delete {}", p.display())));
        std::fs::remove_file(p)?;
        Ok(())
    }
}
