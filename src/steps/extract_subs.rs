use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct ExtractSubsStep;

#[async_trait]
impl Step for ExtractSubsStep {
    fn name(&self) -> &'static str { "extract.subs" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let lang = with.get("language").and_then(|v| v.as_str()).unwrap_or("eng");
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension(format!("{lang}.srt"));
        on_progress(StepProgress::Log(format!("extracting {lang} subs → {}", dest.display())));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"]).arg(&src)
            .args(["-map", &format!("0:s:m:language:{lang}?")])
            .arg(&dest)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().await?;
        if !status.success() { anyhow::bail!("extract.subs ffmpeg failed"); }
        Ok(())
    }
}
