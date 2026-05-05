use super::{Step, StepProgress};
use crate::flow::{staging, Context};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::process::Command;

pub struct RemuxStep;

#[async_trait]
impl Step for RemuxStep {
    fn name(&self) -> &'static str {
        "remux"
    }

    fn executor(&self) -> crate::steps::Executor {
        crate::steps::Executor::Any
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::remux_schema())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let container = with
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("remux: missing `container`"))?;
        let (src, dest) = staging::next_io(ctx, container);
        let _ = std::fs::remove_file(&dest);
        on_progress(StepProgress::Log(format!("remux → {}", dest.display())));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"])
            .arg(&src)
            .args(["-c", "copy"])
            .arg(&dest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("remux ffmpeg failed");
        }
        staging::record_output(ctx, &dest, json!({}));
        Ok(())
    }
}
