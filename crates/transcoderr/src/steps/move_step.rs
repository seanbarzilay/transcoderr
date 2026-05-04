use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

pub struct MoveStep;

#[async_trait]
impl Step for MoveStep {
    fn name(&self) -> &'static str {
        "move"
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let dest = with
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("move: missing `to`"))?;
        let src = std::path::Path::new(&ctx.file.path);
        let dest_path = std::path::Path::new(dest).join(src.file_name().unwrap_or_default());
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        on_progress(StepProgress::Log(format!(
            "move {} -> {}",
            src.display(),
            dest_path.display()
        )));
        std::fs::rename(src, &dest_path).or_else(|_| {
            std::fs::copy(src, &dest_path)?;
            std::fs::remove_file(src)?;
            Ok::<_, std::io::Error>(())
        })?;
        ctx.file.path = dest_path.to_string_lossy().to_string();
        Ok(())
    }
}
