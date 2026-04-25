use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub struct ProbeStep;

#[async_trait]
impl Step for ProbeStep {
    fn name(&self) -> &'static str { "probe" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let path = Path::new(&ctx.file.path);
        on_progress(StepProgress::Log(format!("probing {}", path.display())));
        let v = ffprobe_json(path).await?;
        ctx.probe = Some(v);
        let meta = std::fs::metadata(path)?;
        ctx.file.size_bytes = Some(meta.len());
        Ok(())
    }
}
