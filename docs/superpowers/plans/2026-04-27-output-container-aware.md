# Container-aware `output:replace` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `output:replace` read the planned container and align the final filename's extension with it; delete the source on extension change. Drop the iso-specific `ctx.file.path` mutation + `replaced_input_path` indirection introduced in v0.8.1; `ctx.file.path` stays as the user's original input throughout the flow.

**Architecture:** Three coordinated changes across `output.rs`, `iso_extract.rs`, and `probe.rs`. The `output.rs` and `iso_extract.rs` changes are coupled — landing one without the other would either leave orphan ISOs on disk or overwrite the source file with mkv content. They go in one atomic commit. The `probe.rs` simplification (strip the `bluray:` prefix instead of consulting an `iso_extract` bookkeeping key) is independent and lands first.

**Tech Stack:** Rust workspace (`crates/transcoderr`), `tempfile` for hermetic tests.

**Spec:** `docs/superpowers/specs/2026-04-27-output-container-aware-design.md`

---

## File Structure

```
crates/transcoderr/src/steps/probe.rs            [modify: simplify metadata_path_for + tests]
crates/transcoderr/src/steps/output.rs           [modify: container-aware rename + delete + tests]
crates/transcoderr/src/steps/iso_extract.rs      [modify: drop ctx.file.path mutation, replaced_input_path, target_extension + tests]
```

The `output.rs` + `iso_extract.rs` changes are coupled (see Task 2 preamble). `probe.rs` is independent and lands first.

---

## Task 1: Simplify `probe.rs::metadata_path_for`

**Files:**
- Modify: `crates/transcoderr/src/steps/probe.rs`

Today, `metadata_path_for` consults `ctx.steps["iso_extract"]["replaced_input_path"]` to recover the underlying path for `bluray:` URLs. After Task 2 that key never gets set; the simplification is to strip the `"bluray:"` prefix directly. The change is independent of Task 2 because:

- For non-`bluray:` inputs, both implementations return the input unchanged.
- For `bluray:` inputs while the v0.8.1 iso flow is still in effect, the prefix strip (`bluray:/movies/Unlocked.iso → /movies/Unlocked.iso`) yields the same path the old fallback returned (since iso.extract today writes `replaced_input_path = "/movies/Unlocked.iso"`).

So Task 1 is a no-op refactor at the production level; it's just decoupling probe from iso_extract's bookkeeping.

- [ ] **Step 1: Replace the function body**

In `crates/transcoderr/src/steps/probe.rs`, find:

```rust
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
```

Replace with:

```rust
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
```

Note the `_ctx: &Context` parameter — the signature is preserved for callers (the function is called from `ProbeStep::execute`), but the body no longer reads from it.

- [ ] **Step 2: Update the tests**

In `crates/transcoderr/src/steps/probe.rs`, find the existing `mod tests` block and update it. Today's three tests are:

```rust
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
```

Replace the entire `mod tests` block with:

```rust
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
```

Changes:

- `serde_json::json` import is dropped (no test seeds `ctx.steps` anymore).
- `metadata_path_for_bluray_url_falls_back_to_replaced_input` is renamed to `metadata_path_for_bluray_url_strips_protocol_prefix` and the test body no longer touches `ctx.steps`.
- `metadata_path_for_bluray_url_with_no_replaced_input_returns_url_as_string` is deleted — the "edge case" it covered (URL but no iso_extract key) is now the normal path; the prefix strip handles it directly.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p transcoderr --lib metadata_path_for 2>&1 | tail -10`
Expected: 2 tests pass (`metadata_path_for_plain_path_returns_input`, `metadata_path_for_bluray_url_strips_protocol_prefix`).

- [ ] **Step 4: Run a wider check**

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`. The pre-existing `metrics` integration-test flake reproduces under workspace tests — that's known and not introduced by this change.

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/output-container-aware
git add crates/transcoderr/src/steps/probe.rs
git commit -m "refactor(probe): metadata_path_for strips bluray: prefix instead of consulting iso_extract"
```

---

## Task 2: Container-aware `output:replace` + simplified `iso.extract` (atomic)

**Files:**
- Modify: `crates/transcoderr/src/steps/output.rs` (full body rewrite + 4 tests)
- Modify: `crates/transcoderr/src/steps/iso_extract.rs` (full body rewrite + 2 tests)

**Why these two files commit together:** the changes are coupled. If `iso_extract.rs` is updated first (stops mutating `ctx.file.path` to `Movie.mkv`), then today's `output.rs` would rename the staged `.mkv` tmp to `Movie.iso`'s real path — overwriting the source ISO with mkv content. If `output.rs` is updated first (reads `plan.container`, computes `swap_extension`), today's `iso_extract.rs` already mutates `ctx.file.path` to `Movie.mkv`, so `swap_extension(Movie.mkv, "mkv") == Movie.mkv` and no source-deletion fires — `Movie.iso` lingers on disk. Landing both together avoids both broken intermediates.

- [ ] **Step 1: Replace `output.rs` body**

Replace the entire content of `crates/transcoderr/src/steps/output.rs`:

```rust
use super::{Step, StepProgress};
use crate::flow::Context;
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

fn plan_container(ctx: &Context) -> Option<String> {
    ctx.steps
        .get("_plan")
        .and_then(|v| v.get("container"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
```

Notes:
- `swap_extension` moves here from `iso_extract.rs` (its only caller after this commit).
- The two iso-flavored tests from v0.8.1 (`replace_deletes_replaced_input_when_iso_extract_ran`, `replace_skips_iso_delete_when_not_set`) are gone — the mechanism they exercised no longer exists. The first test's intent is now covered by `replace_swaps_extension_and_deletes_source`, which uses an mp4 source instead of an iso (the swap-extension-and-delete logic is the same regardless of input extension).

- [ ] **Step 2: Replace `iso_extract.rs` body**

Replace the entire content of `crates/transcoderr/src/steps/iso_extract.rs`:

```rust
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
```

Removed (compared to v0.8.1):
- `swap_extension` helper (moved to `output.rs`).
- The `with.target_extension` parameter handling (becomes `_with` since the parameter is unused).
- The `let new_path = swap_extension(...); ctx.file.path = new_path;` block.
- The `ctx.steps.insert("iso_extract", json!({"replaced_input_path": ...}));` recording.
- The `swap_extension_replaces_iso` test (the helper moved; the path math is exercised through `replace_swaps_extension_and_deletes_source` in `output.rs`).
- The `assert_eq!(ctx.file.path, "/movies/Unlocked.mkv")` line in `step_records_bluray_url_for_iso_input` (now asserts UNCHANGED input path).

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build. The `serde_json::json` macro import in `iso_extract.rs` is still used (for `json!({})` in the `record_output` call); the parameter rename `_with` quiets the unused-param warning.

- [ ] **Step 4: Run output + iso_extract tests**

Run: `cargo test -p transcoderr --lib output 2>&1 | tail -15`
Expected: 4 tests pass (`replace_in_place_when_extensions_match`, `replace_swaps_extension_and_deletes_source`, `replace_no_plan_falls_back_to_in_place_rename`, `replace_renames_staged_to_original`).

Run: `cargo test -p transcoderr --lib iso_extract 2>&1 | tail -10`
Expected: 2 tests pass (`step_skips_non_iso_input`, `step_records_bluray_url_for_iso_input`).

- [ ] **Step 5: Run a wider check to catch any caller regressions**

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`. The same pre-existing metrics integration-test flake may surface — that's not introduced by this branch.

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print: feature/output-container-aware
git add crates/transcoderr/src/steps/output.rs crates/transcoderr/src/steps/iso_extract.rs
git commit -m "$(cat <<'EOF'
refactor(output, iso-extract): unify extension-swap into output:replace

output:replace becomes container-aware — reads plan.container, computes
swap_extension(ctx.file.path, container) for the rename target, and
best-effort-deletes the source on extension change.

iso.extract simplifies: no more ctx.file.path mutation, no more
iso_extract.replaced_input_path bookkeeping, no more target_extension
YAML parameter. Body is just gate + URL rewrite.

The two changes are coupled — landing one without the other would
either leave orphan ISOs on disk or overwrite the source with mkv
content. They go in this single commit.
EOF
)"
```

---

## Task 3: Verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm branch and clean state**

Run: `git branch --show-current && git status --short`
Expected: `feature/output-container-aware`, no uncommitted changes.

- [ ] **Step 2: Run the full per-crate test suite**

Run: `cargo test -p transcoderr --locked --lib --tests 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok` lines. The pre-existing metrics flake may surface here too.

- [ ] **Step 3: Confirm dead-code paths are gone**

Run:

```bash
grep -n 'replaced_input_path' crates/transcoderr/src/steps/*.rs || echo "(no matches — clean)"
grep -n 'ctx.file.path = ' crates/transcoderr/src/steps/iso_extract.rs || echo "(no matches — clean)"
grep -n 'swap_extension' crates/transcoderr/src/steps/iso_extract.rs || echo "(no matches — clean)"
```

Expected: each grep prints the "no matches" line.

- [ ] **Step 4: Confirm `swap_extension` lives only in `output.rs`**

Run: `grep -rn 'fn swap_extension' crates/transcoderr/src/`
Expected output:
```
crates/transcoderr/src/steps/output.rs:??:fn swap_extension(path: &str, new_ext: &str) -> String {
```
(One match; the line number isn't important.)

- [ ] **Step 5: Branch commit list**

Run: `git log --oneline feature/output-container-aware ^main`
Expected (in some order, plus the spec commit `995e8dc` already on the branch):

```
refactor(output, iso-extract): unify extension-swap into output:replace
refactor(probe): metadata_path_for strips bluray: prefix instead of consulting iso_extract
docs(spec): container-aware output:replace
```

- [ ] **Step 6: Optional manual end-to-end on the dev server**

If reachable: deploy the binary, then trigger two webhook runs:

1. An `.mp4` source through `hevc-normalize`. Expected: lands at `<stem>.mkv`, the `.mp4` is gone after success.
2. An `.iso` source through `hevc-normalize`. Expected: lands at `<stem>.mkv`, the `.iso` is gone after success. (Same outward behavior as v0.8.1 — only the internal mechanism changed.)

Watch the run events: the `output` step's log line should now read
`"replacing /movies/Movie.mp4 with .../tcr-00.tmp.mkv -> /movies/Movie.mkv"`
followed by either `"removed source /movies/Movie.mp4"` (extension change) or no second log line (in-place rename).

If the dev server isn't reachable, skip — the per-crate tests above cover everything that can be verified hermetically. End-to-end can be confirmed against the next ingested file.

- [ ] **Step 7: (No commit — verification only.)** The branch is ready for review/merge.
