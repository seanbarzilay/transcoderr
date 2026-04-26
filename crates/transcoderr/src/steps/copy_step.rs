use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct CopyStep;

#[async_trait]
impl Step for CopyStep {
    fn name(&self) -> &'static str { "copy" }

    async fn execute(
        &self, with: &BTreeMap<String, Value>, ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let dest = with.get("to").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("copy: missing `to`"))?;
        let src = std::path::Path::new(&ctx.file.path);
        let dest_path = std::path::Path::new(dest).join(src.file_name().unwrap_or_default());
        if let Some(parent) = dest_path.parent() { std::fs::create_dir_all(parent)?; }
        on_progress(StepProgress::Log(format!("copy {} -> {}", src.display(), dest_path.display())));
        std::fs::copy(src, &dest_path)?;
        Ok(())
    }
}
