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

        // Skip silently if probe shows no matching subtitle stream. Otherwise ffmpeg
        // refuses with "Output file does not contain any stream" when there's nothing
        // to extract.
        let has_sub = ctx
            .probe
            .as_ref()
            .and_then(|p| p.get("streams"))
            .and_then(|s| s.as_array())
            .map(|arr| {
                arr.iter().any(|s| {
                    let is_sub = s
                        .get("codec_type")
                        .and_then(|v| v.as_str())
                        == Some("subtitle");
                    let s_lang = s
                        .get("tags")
                        .and_then(|t| t.get("language"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    is_sub && (s_lang == lang || s_lang.is_empty())
                })
            })
            .unwrap_or(false);
        if !has_sub {
            on_progress(StepProgress::Log(format!(
                "extract.subs: no {lang} subtitle stream found; skipping"
            )));
            return Ok(());
        }

        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension(format!("{lang}.srt"));
        on_progress(StepProgress::Log(format!(
            "extracting {lang} subs → {}",
            dest.display()
        )));
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-y", "-i"])
            .arg(&src)
            .args(["-map", &format!("0:s:m:language:{lang}?")])
            .arg(&dest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("extract.subs ffmpeg failed");
        }
        Ok(())
    }
}
