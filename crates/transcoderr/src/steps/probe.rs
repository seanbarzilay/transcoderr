use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::{staging, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ProbeStep;

/// Resolve the path that should be passed to `fs::metadata`. For a
/// `bluray:` URL chain head, strip the protocol prefix to recover the
/// underlying ISO path. For any other input, return it unchanged.
pub(crate) fn metadata_path_for(input: &str, _ctx: &Context) -> String {
    if let Some(real) = input.strip_prefix("bluray:") {
        real.to_string()
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

    #[test]
    fn metadata_path_for_plain_path_returns_input() {
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(metadata_path_for("/m/Dune.mkv", &ctx), "/m/Dune.mkv");
    }

    #[test]
    fn metadata_path_for_bluray_url_strips_protocol_prefix() {
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(
            metadata_path_for("bluray:/movies/Unlocked.iso", &ctx),
            "/movies/Unlocked.iso"
        );
    }
}
