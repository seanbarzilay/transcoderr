use super::{Step, StepProgress};
use crate::flow::{plan::load_plan, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct OutputStep;

#[async_trait]
impl Step for OutputStep {
    fn name(&self) -> &'static str { "output" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let mode = with.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
        if mode != "replace" {
            anyhow::bail!("Phase 1 only supports mode=replace, got {:?}", mode);
        }
        let staged = ctx
            .steps
            .get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no transcode output_path in context"))?
            .to_string();

        let original = ctx.file.path.clone();

        // If a plan exists, the planned container determines the final
        // extension. mp4 sources transcoded to mkv land at <stem>.mkv
        // and the .mp4 is deleted. Same-extension flows (mkv -> mkv)
        // keep today's atomic in-place rename. No plan -> no extension
        // swap.
        let final_path = match plan_container(ctx) {
            Some(container) => swap_extension(&original, &container),
            None => original.clone(),
        };

        on_progress(StepProgress::Log(format!(
            "replacing {original} with {staged} -> {final_path}"
        )));
        std::fs::rename(&staged, &final_path)?;

        // Best-effort delete of the source when the final path differs
        // (extension change). The new file is already in place, so a
        // delete failure is non-fatal -- we log and continue.
        if final_path != original {
            match std::fs::remove_file(&original) {
                Ok(()) => on_progress(StepProgress::Log(format!(
                    "removed source {original}"
                ))),
                Err(e) => on_progress(StepProgress::Log(format!(
                    "warn: failed to delete source {original}: {e}"
                ))),
            }
        }
        Ok(())
    }
}

/// Pull the planned container ext (e.g. "mkv") from `ctx.steps["_plan"]`.
/// Goes through `load_plan` rather than a raw JSON walk so that any
/// future `serde` rename/transform on `StreamPlan` is automatically
/// respected — a raw `v.get("container")` lookup would silently break
/// if `StreamPlan` ever gained `#[serde(rename_all = "camelCase")]`.
fn plan_container(ctx: &Context) -> Option<String> {
    load_plan(ctx).map(|p| p.container)
}

/// Replace the trailing extension on `path` with `new_ext`. Used to
/// align the output filename with the planned container.
fn swap_extension(path: &str, new_ext: &str) -> String {
    let pb = PathBuf::from(path);
    let parent = pb.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let stem = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    parent.join(format!("{stem}.{new_ext}")).to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    fn seed_plan(ctx: &mut Context, container: &str) {
        ctx.steps.insert(
            "_plan".into(),
            json!({"container": container}),
        );
    }

    #[tokio::test]
    async fn replace_in_place_when_extensions_match() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original).unwrap().write_all(b"old").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"new").unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&original).unwrap(), b"new",
            "in-place atomic rename should overwrite original with staged content");
    }

    #[tokio::test]
    async fn replace_swaps_extension_and_deletes_source() {
        let dir = tempdir().unwrap();
        let source_mp4 = dir.path().join("Movie.mp4");
        let final_mkv = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mp4.tcr-00.tmp.mkv");
        std::fs::File::create(&source_mp4).unwrap().write_all(b"mp4 bytes").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"mkv bytes").unwrap();

        let mut ctx = Context::for_file(source_mp4.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert!(!source_mp4.exists(), "source .mp4 should be deleted on extension change");
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"mkv bytes",
            "the .mkv should land at the swapped-extension path");
    }

    #[tokio::test]
    async fn replace_no_plan_falls_back_to_in_place_rename() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original).unwrap().write_all(b"old").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"new").unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
        // NO _plan key — this test exercises the no-plan fallback.
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&original).unwrap(), b"new",
            "no plan -> verbatim rename to ctx.file.path");
        assert!(original.exists(), "original path still exists (in-place rename)");
    }

    #[tokio::test]
    async fn replace_renames_staged_to_original() {
        // Tweaked from the v0.8.1 version: now seeds _plan { container: "mkv" }
        // matching the source's .mkv extension. Equivalent to the in-place
        // case above but kept as a more general "the staged content lands at
        // the destination path" assertion.
        let dir = tempdir().unwrap();
        let original = dir.path().join("Show.S01E02.mkv");
        let staged = dir.path().join("Show.S01E02.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original).unwrap().write_all(b"old").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"transcoded").unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&original).unwrap(), b"transcoded");
    }
}
