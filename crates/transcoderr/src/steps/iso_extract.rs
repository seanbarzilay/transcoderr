//! `iso.extract` step: detects Blu-ray ISO inputs and rewrites the
//! staging chain head to a `bluray:` URL so ffmpeg (with libbluray) can
//! ingest the disc directly. No on-disk extraction; the step is pure
//! string manipulation and takes <1ms.
//!
//! The output filename's extension is decided later, by `output:replace`,
//! based on the plan's `container` field. iso.extract no longer touches
//! `ctx.file.path` — the user's original input path stays stable
//! throughout the flow.

use crate::flow::{staging, Context};
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub struct IsoExtractStep;

#[async_trait]
impl Step for IsoExtractStep {
    fn name(&self) -> &'static str { "iso.extract" }

    fn executor(&self) -> crate::steps::Executor { crate::steps::Executor::Any }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let input_path = ctx.file.path.clone();
        if !input_path.to_lowercase().ends_with(".iso") {
            on_progress(StepProgress::Log(format!(
                "iso.extract: not an iso, skipping ({input_path})"
            )));
            return Ok(());
        }

        let bluray_url = format!("bluray:{input_path}");
        on_progress(StepProgress::Log(format!(
            "iso.extract: routing as Blu-ray URL: {bluray_url}"
        )));
        staging::record_output(ctx, std::path::Path::new(&bluray_url), json!({}));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn step_skips_non_iso_input() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        let mut log_count = 0usize;
        let mut on_progress = |p: StepProgress| {
            if matches!(p, StepProgress::Log(_)) { log_count += 1; }
        };
        IsoExtractStep
            .execute(&BTreeMap::new(), &mut ctx, &mut on_progress)
            .await
            .expect("ok");
        assert!(ctx.steps.get("transcode").is_none());
        assert_eq!(ctx.file.path, "/m/Dune.mkv");
        assert!(log_count >= 1);
    }

    #[tokio::test]
    async fn step_records_bluray_url_for_iso_input() {
        let mut ctx = Context::for_file("/movies/Unlocked.iso");
        let mut on_progress = |_p: StepProgress| {};
        IsoExtractStep
            .execute(&BTreeMap::new(), &mut ctx, &mut on_progress)
            .await
            .expect("ok");

        // Chain head is the bluray: URL.
        assert_eq!(
            staging::current_input(&ctx),
            "bluray:/movies/Unlocked.iso"
        );
        // ctx.file.path is UNCHANGED (the user's original input).
        assert_eq!(ctx.file.path, "/movies/Unlocked.iso");
        // No iso_extract bookkeeping key — output:replace doesn't need it anymore.
        assert!(ctx.steps.get("iso_extract").is_none());
    }
}
