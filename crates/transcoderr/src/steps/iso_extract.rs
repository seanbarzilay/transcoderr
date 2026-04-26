//! `iso.extract` step: detects Blu-ray ISO inputs and rewrites the
//! staging chain head to a `bluray:` URL so ffmpeg (with libbluray) can
//! ingest the disc directly. No on-disk extraction; the step is pure
//! string manipulation and takes <1ms.

use crate::flow::{staging, Context};
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct IsoExtractStep;

#[async_trait]
impl Step for IsoExtractStep {
    fn name(&self) -> &'static str { "iso.extract" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
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

        let target_extension = with
            .get("target_extension")
            .and_then(|v| v.as_str())
            .unwrap_or("mkv")
            .to_string();

        let bluray_url = format!("bluray:{input_path}");
        on_progress(StepProgress::Log(format!(
            "iso.extract: routing as Blu-ray URL: {bluray_url}"
        )));

        // Put the URL into the staging chain head. Downstream steps that
        // pass it to ffmpeg's `-i` get it as-is; ffmpeg + libbluray handle
        // the bluray: protocol natively.
        staging::record_output(ctx, std::path::Path::new(&bluray_url), json!({}));

        // Record the original ISO path for output:replace to delete on
        // success. Lives in ctx.steps["iso_extract"], NOT in the chain
        // (which gets overwritten by subsequent steps).
        ctx.steps.insert(
            "iso_extract".into(),
            json!({"replaced_input_path": &input_path}),
        );

        // Mutate ctx.file.path to the intended final basename. The .mkv
        // doesn't exist on disk yet — output:replace will atomically
        // rename the transcoded tmp onto it at the end of the flow.
        let new_path = swap_extension(&input_path, &target_extension);
        on_progress(StepProgress::Log(format!(
            "iso.extract: ctx.file.path {input_path} -> {new_path}"
        )));
        ctx.file.path = new_path;

        Ok(())
    }
}

/// Replace the trailing extension on `path` with `new_ext`. Caller is
/// responsible for ensuring `path` ends with `.iso`.
fn swap_extension(path: &str, new_ext: &str) -> String {
    let pb = PathBuf::from(path);
    let parent = pb.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let stem = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    parent.join(format!("{stem}.{new_ext}")).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn step_skips_non_iso_input() {
        let mut ctx = Context::for_file("/m/Dune.mkv");
        let step = IsoExtractStep;
        let mut log_count = 0usize;
        let mut on_progress = |p: StepProgress| {
            if matches!(p, StepProgress::Log(_)) { log_count += 1; }
        };
        let with = BTreeMap::new();
        step.execute(&with, &mut ctx, &mut on_progress).await.expect("ok");
        assert!(ctx.steps.get("transcode").is_none());
        assert!(ctx.steps.get("iso_extract").is_none());
        assert_eq!(ctx.file.path, "/m/Dune.mkv");
        assert!(log_count >= 1);
    }

    #[test]
    fn swap_extension_replaces_iso() {
        assert_eq!(swap_extension("/m/Dune.iso", "mkv"), "/m/Dune.mkv");
        assert_eq!(swap_extension("/movies/Unlocked (2017)/Unlocked.iso", "mkv"),
                   "/movies/Unlocked (2017)/Unlocked.mkv");
    }

    #[tokio::test]
    async fn step_records_bluray_url_for_iso_input() {
        let mut ctx = Context::for_file("/movies/Unlocked.iso");
        let step = IsoExtractStep;
        let mut on_progress = |_p: StepProgress| {};
        let with = BTreeMap::new();
        step.execute(&with, &mut ctx, &mut on_progress).await.expect("ok");

        // Chain head is the bluray: URL.
        assert_eq!(
            staging::current_input(&ctx),
            "bluray:/movies/Unlocked.iso"
        );
        // Original ISO path recorded for output:replace.
        assert_eq!(
            ctx.steps.get("iso_extract")
                .and_then(|s| s.get("replaced_input_path"))
                .and_then(|v| v.as_str()),
            Some("/movies/Unlocked.iso")
        );
        // ctx.file.path swapped to the intended final basename.
        assert_eq!(ctx.file.path, "/movies/Unlocked.mkv");
    }
}
