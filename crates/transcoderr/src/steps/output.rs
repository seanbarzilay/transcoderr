use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

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
        on_progress(StepProgress::Log(format!("replacing {} with {}", original, staged)));

        // Same-filesystem atomic rename. (For Phase 1 we assume staged is sibling of original.)
        std::fs::rename(&staged, &original)?;

        // If iso.extract ran upstream, delete the original ISO it preserved. Best-effort:
        // the .mkv is already in place at this point, so a delete failure is non-fatal.
        if let Some(replaced) = ctx
            .steps
            .get("iso_extract")
            .and_then(|s| s.get("replaced_input_path"))
            .and_then(|v| v.as_str())
        {
            match std::fs::remove_file(replaced) {
                Ok(()) => on_progress(StepProgress::Log(format!(
                    "removed replaced input {replaced}"
                ))),
                Err(e) => on_progress(StepProgress::Log(format!(
                    "warn: failed to delete replaced input {replaced}: {e}"
                ))),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn replace_renames_staged_to_original() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original).unwrap().write_all(b"old").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"new").unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
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
        assert_eq!(std::fs::read(&original).unwrap(), b"new");
    }

    #[tokio::test]
    async fn replace_deletes_replaced_input_when_iso_extract_ran() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("Movie.iso");
        let final_mkv = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-01.tmp.mkv");
        std::fs::File::create(&iso).unwrap().write_all(b"iso bytes").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"mkv bytes").unwrap();

        // Simulate post-iso.extract context state.
        let mut ctx = Context::for_file(final_mkv.to_string_lossy().to_string());
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );
        ctx.steps.insert(
            "iso_extract".into(),
            json!({"replaced_input_path": iso.to_string_lossy()}),
        );

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(!staged.exists(), "staged should be moved");
        assert!(!iso.exists(), "original ISO should be deleted");
        assert_eq!(std::fs::read(&final_mkv).unwrap(), b"mkv bytes");
    }

    #[tokio::test]
    async fn replace_skips_iso_delete_when_not_set() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("Movie.mkv");
        let staged = dir.path().join("Movie.mkv.tcr-00.tmp.mkv");
        std::fs::File::create(&original).unwrap().write_all(b"old").unwrap();
        std::fs::File::create(&staged).unwrap().write_all(b"new").unwrap();

        let mut ctx = Context::for_file(original.to_string_lossy().to_string());
        ctx.steps.insert(
            "transcode".into(),
            json!({"output_path": staged.to_string_lossy()}),
        );
        // No iso_extract entry — should behave exactly like before.

        let mut noop = |_p: StepProgress| {};
        OutputStep
            .execute(&BTreeMap::new(), &mut ctx, &mut noop)
            .await
            .unwrap();

        assert!(original.exists());
    }
}
