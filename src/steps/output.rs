use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct OutputStep;

#[async_trait]
impl Step for OutputStep {
    fn name(&self) -> &'static str { "output" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let mode = with.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
        if mode != "replace" {
            anyhow::bail!("Phase 1 only supports mode=replace, got {:?}", mode);
        }
        let staged = ctx.steps.get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no transcode output_path in context"))?
            .to_string();

        let original = ctx.file.path.clone();
        on_progress(StepProgress::Log(format!("replacing {} with {}", original, staged)));

        // Same-filesystem atomic rename. (For Phase 1 we assume staged is sibling of original.)
        std::fs::rename(&staged, &original)?;
        Ok(())
    }
}
