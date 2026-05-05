use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct VerifyPlayableStep;

#[async_trait]
impl Step for VerifyPlayableStep {
    fn name(&self) -> &'static str {
        "verify.playable"
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::verify_playable_schema())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let min_ratio = with
            .get("min_duration_ratio")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.99);

        let target = ctx
            .steps
            .get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .unwrap_or(&ctx.file.path)
            .to_string();

        on_progress(StepProgress::Log(format!("verifying {target}")));
        let probed = ffprobe_json(Path::new(&target))
            .await
            .map_err(|e| anyhow::anyhow!("verify ffprobe failed: {e}"))?;

        let original_dur = ctx
            .probe
            .as_ref()
            .and_then(|p| p["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let new_dur = probed["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if original_dur > 0.0 && (new_dur / original_dur) < min_ratio {
            anyhow::bail!(
                "verify failed: new={new_dur:.2}s vs original={original_dur:.2}s (<{:.2}x)",
                min_ratio
            );
        }
        Ok(())
    }
}
