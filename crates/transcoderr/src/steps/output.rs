use super::{Step, StepProgress};
use crate::flow::{plan::load_plan, Context};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct OutputStep;

#[async_trait]
impl Step for OutputStep {
    fn name(&self) -> &'static str {
        "output"
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::output_schema())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let mode = OutputMode::parse(with)?;
        let staged = ctx
            .steps
            .get("transcode")
            .and_then(|v| v.get("output_path"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("no transcode output_path in context"))?
            .to_string();

        let original = ctx.file.path.clone();

        let final_path = match mode {
            OutputMode::Replace => replacement_path(ctx, &original),
            OutputMode::Alongside => alongside_path(ctx, &original, &staged),
        };

        match mode {
            OutputMode::Replace => {
                on_progress(StepProgress::Log(format!(
                    "replacing {original} with {staged} -> {final_path}"
                )));
                std::fs::rename(&staged, &final_path)?;

                // Best-effort delete of the source when the final path differs
                // (extension change). The new file is already in place, so a
                // delete failure is non-fatal -- we log and continue.
                if final_path != original {
                    match std::fs::remove_file(&original) {
                        Ok(()) => {
                            on_progress(StepProgress::Log(format!("removed source {original}")))
                        }
                        Err(e) => on_progress(StepProgress::Log(format!(
                            "warn: failed to delete source {original}: {e}"
                        ))),
                    }
                }
            }
            OutputMode::Alongside => {
                on_progress(StepProgress::Log(format!(
                    "keeping {original}; moving {staged} -> {final_path}"
                )));
                std::fs::rename(&staged, &final_path)?;
            }
        }

        // Downstream steps (notify → jellyfin, etc.) read ctx.file.path
        // to reference the on-disk file. Without this update they'd see
        // the pre-rename .mp4 path after a .mp4 → .mkv container swap.
        ctx.file.path = final_path;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Replace,
    Alongside,
}

impl OutputMode {
    fn parse(with: &BTreeMap<String, Value>) -> anyhow::Result<Self> {
        match with
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace")
        {
            "replace" => Ok(Self::Replace),
            "alongside" => Ok(Self::Alongside),
            mode => anyhow::bail!(
                "output: supported modes are replace and alongside, got {:?}",
                mode
            ),
        }
    }
}

fn replacement_path(ctx: &Context, original: &str) -> String {
    // If a plan exists, the planned container determines the final
    // extension. mp4 sources transcoded to mkv land at <stem>.mkv
    // and the .mp4 is deleted. Same-extension flows (mkv -> mkv)
    // keep today's atomic in-place rename. No plan -> no extension
    // swap.
    match plan_container(ctx) {
        Some(container) => swap_extension(original, &container),
        None => original.to_string(),
    }
}

fn alongside_path(ctx: &Context, original: &str, staged: &str) -> String {
    let ext = plan_container(ctx)
        .or_else(|| path_extension(staged))
        .or_else(|| path_extension(original))
        .unwrap_or_else(|| "mkv".to_string());
    let preferred = swap_extension(original, &ext);
    if preferred != original && !Path::new(&preferred).exists() {
        return preferred;
    }
    unique_transcoderr_path(&preferred, original, &ext)
}

/// Pull the planned container ext (e.g. "mkv") from `ctx.steps["_plan"]`.
/// Goes through `load_plan` rather than a raw JSON walk so that any
/// future `serde` rename/transform on `StreamPlan` is automatically
/// respected — a raw `v.get("container")` lookup would silently break
/// if `StreamPlan` ever gained `#[serde(rename_all = "camelCase")]`.
fn plan_container(ctx: &Context) -> Option<String> {
    load_plan(ctx).and_then(|p| normalize_ext(&p.container))
}

fn path_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .and_then(normalize_ext)
}

fn normalize_ext(ext: &str) -> Option<String> {
    let ext = ext.trim().trim_start_matches('.');
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_string())
    }
}

/// Replace the trailing extension on `path` with `new_ext`. Used to
/// align the output filename with the planned container.
fn swap_extension(path: &str, new_ext: &str) -> String {
    let new_ext = normalize_ext(new_ext).unwrap_or_else(|| "mkv".to_string());
    let pb = PathBuf::from(path);
    let parent = pb
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    parent
        .join(format!("{stem}.{new_ext}"))
        .to_string_lossy()
        .into_owned()
}

fn unique_transcoderr_path(preferred: &str, original: &str, ext: &str) -> String {
    let ext = normalize_ext(ext).unwrap_or_else(|| "mkv".to_string());
    let pb = PathBuf::from(preferred);
    let parent = pb
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    for i in 0u32.. {
        let suffix = if i == 0 {
            "transcoderr".to_string()
        } else {
            format!("transcoderr-{i}")
        };
        let candidate = parent
            .join(format!("{stem}.{suffix}.{ext}"))
            .to_string_lossy()
            .into_owned();
        if candidate != original && !Path::new(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("unbounded candidate loop must return")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    fn seed_plan(ctx: &mut Context, container: &str) {
        // Go through `save_plan` (rather than seeding raw JSON) so the
        // shape in ctx.steps["_plan"] is a real serialized StreamPlan.
        // This keeps these tests honest if StreamPlan ever gains new
        // required fields or `#[serde(rename_all = ...)]`.
        let plan = crate::flow::plan::StreamPlan {
            container: container.to_string(),
            ..Default::default()
        };
        crate::flow::plan::save_plan(ctx, &plan);
    }

    fn alongside_with() -> BTreeMap<String, Value> {
        BTreeMap::from([("mode".to_string(), json!("alongside"))])
    }

    #[tokio::test]
    async fn replace_in_place_when_extensions_match() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"new")
            .unwrap();

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
        assert_eq!(
            std::fs::read(&original).unwrap(),
            b"new",
            "in-place atomic rename should overwrite original with staged content"
        );
    }

    #[tokio::test]
    async fn replace_swaps_extension_and_deletes_source() {
        let dir = tempdir().unwrap();
        let source_mp4 = dir.path().join("Movie.mp4");
        let final_mkv = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mp4.tcr-00.tmp.mkv");
        std::fs::File::create(&source_mp4)
            .unwrap()
            .write_all(b"mp4 bytes")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"mkv bytes")
            .unwrap();

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
        assert!(
            !source_mp4.exists(),
            "source .mp4 should be deleted on extension change"
        );
        assert_eq!(
            std::fs::read(&final_mkv).unwrap(),
            b"mkv bytes",
            "the .mkv should land at the swapped-extension path"
        );
        assert_eq!(
            ctx.file.path,
            final_mkv.to_string_lossy(),
            "ctx.file.path must point at the new .mkv so downstream steps see the right path"
        );
    }

    #[tokio::test]
    async fn replace_no_plan_falls_back_to_in_place_rename() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"new")
            .unwrap();

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
        assert_eq!(
            std::fs::read(&original).unwrap(),
            b"new",
            "no plan -> verbatim rename to ctx.file.path"
        );
        assert!(
            original.exists(),
            "original path still exists (in-place rename)"
        );
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
        std::fs::File::create(&original)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"transcoded")
            .unwrap();

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

    #[tokio::test]
    async fn alongside_keeps_source_and_writes_planned_container_path() {
        let dir = tempdir().unwrap();
        let source_mp4 = dir.path().join("Movie.mp4");
        let final_mkv = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mp4.tcr-00.tmp.mkv");
        std::fs::File::create(&source_mp4)
            .unwrap()
            .write_all(b"mp4 bytes")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"mkv bytes")
            .unwrap();

        let mut ctx = Context::for_file(source_mp4.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&alongside_with(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(
            std::fs::read(&source_mp4).unwrap(),
            b"mp4 bytes",
            "source must be left untouched"
        );
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"mkv bytes");
        assert_eq!(ctx.file.path, final_mkv.to_string_lossy());
    }

    #[tokio::test]
    async fn alongside_uses_transcoderr_suffix_when_source_extension_matches() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let final_mkv = dir.path().join("Movie.transcoderr.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"new")
            .unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&alongside_with(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&original).unwrap(), b"old");
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"new");
        assert_eq!(ctx.file.path, final_mkv.to_string_lossy());
    }

    #[tokio::test]
    async fn alongside_avoids_existing_container_path() {
        let dir = tempdir().unwrap();
        let source_mp4 = dir.path().join("Movie.mp4");
        let existing_mkv = dir.path().join("Movie.mkv");
        let final_mkv = dir.path().join("Movie.transcoderr.mkv");
        let staged = dir.path().join("Movie.mp4.tcr-00.tmp.mkv");
        std::fs::File::create(&source_mp4)
            .unwrap()
            .write_all(b"mp4 bytes")
            .unwrap();
        std::fs::File::create(&existing_mkv)
            .unwrap()
            .write_all(b"existing")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"mkv bytes")
            .unwrap();

        let mut ctx = Context::for_file(source_mp4.to_string_lossy().to_string());
        seed_plan(&mut ctx, "mkv");
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&alongside_with(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&source_mp4).unwrap(), b"mp4 bytes");
        assert_eq!(std::fs::read(&existing_mkv).unwrap(), b"existing");
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"mkv bytes");
        assert_eq!(ctx.file.path, final_mkv.to_string_lossy());
    }

    #[tokio::test]
    async fn alongside_without_plan_uses_staged_extension() {
        let dir = tempdir().unwrap();
        let source_avi = dir.path().join("Movie.avi");
        let final_mkv = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.avi.tcr-00.tmp.mkv");
        std::fs::File::create(&source_avi)
            .unwrap()
            .write_all(b"avi bytes")
            .unwrap();
        std::fs::File::create(&staged)
            .unwrap()
            .write_all(b"mkv bytes")
            .unwrap();

        let mut ctx = Context::for_file(source_avi.to_string_lossy().to_string());
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&alongside_with(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert_eq!(std::fs::read(&source_avi).unwrap(), b"avi bytes");
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"mkv bytes");
        assert_eq!(ctx.file.path, final_mkv.to_string_lossy());
    }
}
