use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct RemuxStep;

#[async_trait]
impl Step for RemuxStep {
    fn name(&self) -> &'static str { "remux" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let container = with.get("container").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("remux: missing `container`"))?;
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension(format!("transcoderr.tmp.{container}"));
        let _ = std::fs::remove_file(&dest);
        on_progress(StepProgress::Log(format!("remux → {}", dest.display())));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"]).arg(&src)
            .args(["-c", "copy"]).arg(&dest)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().await?;
        if !status.success() { anyhow::bail!("remux ffmpeg failed"); }
        ctx.record_step_output("transcode", json!({ "output_path": dest.to_string_lossy() }));
        Ok(())
    }
}
