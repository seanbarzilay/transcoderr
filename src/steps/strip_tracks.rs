use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct StripTracksStep;

#[async_trait]
impl Step for StripTracksStep {
    fn name(&self) -> &'static str { "strip.tracks" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let langs = with.get("keep_audio_languages")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_else(|| vec!["eng".into()]);
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-y", "-i"]).arg(&src);
        cmd.args(["-map", "0:v", "-c:v", "copy"]);
        for l in &langs {
            cmd.args(["-map", &format!("0:a:m:language:{l}?"), "-c:a", "copy"]);
        }
        cmd.args(["-map", "0:s?", "-c:s", "copy"]).arg(&dest);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        on_progress(StepProgress::Log(format!("strip tracks: keep audio {langs:?}")));
        let status = cmd.status().await?;
        if !status.success() { anyhow::bail!("strip.tracks ffmpeg failed"); }
        ctx.record_step_output("transcode", json!({ "output_path": dest.to_string_lossy() }));
        Ok(())
    }
}
