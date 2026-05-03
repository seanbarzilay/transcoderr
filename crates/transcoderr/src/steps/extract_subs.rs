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

    fn executor(&self) -> crate::steps::Executor { crate::steps::Executor::Any }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let lang = with.get("language").and_then(|v| v.as_str()).unwrap_or("eng");

        // Only text-format subtitle codecs can be written into an .srt file. Bitmap
        // formats (PGS, DVD, DVB) require OCR and aren't supported here. Skip silently
        // when there's no matching text-format stream — Blu-ray rips usually have
        // language-tagged PGS subs which would match a naive language filter but
        // ffmpeg can't write them to .srt and bails.
        const TEXT_SUB_CODECS: &[&str] = &["srt", "subrip", "ass", "ssa", "mov_text", "webvtt"];

        let mut found_lang_match = false;
        let has_text_sub = ctx
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
                    if !is_sub {
                        return false;
                    }
                    let s_lang = s
                        .get("tags")
                        .and_then(|t| t.get("language"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if s_lang != lang {
                        return false;
                    }
                    found_lang_match = true;
                    let codec = s
                        .get("codec_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    TEXT_SUB_CODECS.contains(&codec.as_str())
                })
            })
            .unwrap_or(false);
        if !has_text_sub {
            let why = if found_lang_match {
                format!("only bitmap {lang} subtitle streams found (PGS/DVD/DVB); cannot write .srt")
            } else {
                format!("no {lang} subtitle stream found")
            };
            on_progress(StepProgress::Log(format!("extract.subs: {why}; skipping")));
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
            // See strip_tracks.rs for why `:?` (not bare `?`) is the
            // right form on ffmpeg 7.1+.
            .args(["-map", &format!("0:s:m:language:{lang}:?")])
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
