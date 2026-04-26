use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::{staging, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ProbeStep;

/// Resolve the path that should be passed to `fs::metadata`. For a
/// `bluray:` URL chain head, fall back to the original on-disk ISO recorded
/// by `iso.extract` so `size_bytes` reflects the user's actual file. For
/// any other input (a real filesystem path), return it unchanged.
pub(crate) fn metadata_path_for(input: &str, ctx: &Context) -> String {
    if input.starts_with("bluray:") {
        ctx.steps
            .get("iso_extract")
            .and_then(|s| s.get("replaced_input_path"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| input.to_string())
    } else {
        input.to_string()
    }
}

#[async_trait]
impl Step for ProbeStep {
    fn name(&self) -> &'static str { "probe" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let input = staging::current_input(ctx).to_string();
        let path = Path::new(&input);
        on_progress(StepProgress::Log(format!("probing {}", path.display())));
        let v = ffprobe_json(path).await?;
        ctx.probe = Some(v);
        let metadata_path = metadata_path_for(&input, ctx);
        let meta = std::fs::metadata(Path::new(&metadata_path))?;
        ctx.file.size_bytes = Some(meta.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_path_for_plain_path_returns_input() {
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(metadata_path_for("/m/Dune.mkv", &ctx), "/m/Dune.mkv");
    }

    #[test]
    fn metadata_path_for_bluray_url_falls_back_to_replaced_input() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        ctx.steps.insert(
            "iso_extract".into(),
            json!({"replaced_input_path": "/movies/Unlocked.iso"}),
        );
        assert_eq!(
            metadata_path_for("bluray:/movies/Unlocked.iso", &ctx),
            "/movies/Unlocked.iso"
        );
    }

    #[test]
    fn metadata_path_for_bluray_url_with_no_replaced_input_returns_url_as_string() {
        // Edge case: a flow set the chain head to a bluray: URL without
        // going through iso.extract. We don't pretend to handle this
        // gracefully — return the URL itself, let fs::metadata fail
        // loudly so the operator sees the misconfiguration.
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(
            metadata_path_for("bluray:/movies/Unlocked.iso", &ctx),
            "bluray:/movies/Unlocked.iso"
        );
    }
}
