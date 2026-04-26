# ISO via libbluray Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the v0.8.0 7z-based body of `iso.extract` with a URL-rewrite that hands the ISO to ffmpeg's `bluray:` protocol; teach `probe` to fall back to the original ISO's path for `fs::metadata` when the chain head is a `bluray:` URL; delete the now-unused 7z parser/helpers/fixture/Dockerfile-installs.

**Architecture:** The staging chain head transitions from "always a real filesystem path" to "either a real path OR a URL string". `iso.extract` writes `bluray:<original-iso-path>` into the chain instead of demuxing to a sibling `.m2ts`. Every downstream consumer that hands the chain head to ffmpeg's `-i` works unchanged (ffmpeg handles `bluray:` natively via libbluray, which is already in the runtime image). The single non-ffmpeg consumer — `probe.rs::fs::metadata` — gets a small string-prefix branch that falls back to the original ISO's recorded path.

**Tech Stack:** Rust workspace (`crates/transcoderr`), `tokio` async, `ffmpeg` + `libbluray` (already in `jrottenberg/ffmpeg:6.0-nvidia2204`).

**Spec:** `docs/superpowers/specs/2026-04-26-iso-bluray-design.md`

---

## File Structure

```
crates/transcoderr/src/steps/probe.rs                            [modify: bluray: URL fallback]
crates/transcoderr/src/steps/iso_extract.rs                      [modify: replace 7z body + tests]
crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt            [delete]
crates/transcoderr/src/flow/engine.rs                            [modify: drop iso.extract from timeout list]
docker/Dockerfile.cpu                                            [modify: remove p7zip-full]
docker/Dockerfile.intel                                          [modify: remove p7zip-full]
docker/Dockerfile.nvidia                                         [modify: remove p7zip-full]
docker/Dockerfile.full                                           [modify: remove p7zip-full]
```

---

## Task 1: Probe `size_bytes` fallback for `bluray:` URL inputs

**Files:**
- Modify: `crates/transcoderr/src/steps/probe.rs`

The current probe step calls `fs::metadata` on whatever path comes out of the staging chain. Once `iso.extract` (Task 2) writes a `bluray:` URL into the chain, that metadata call would fail. Pull the path-resolution into a small pure function so we can unit-test both branches (URL fallback + plain path) without invoking ffprobe.

- [ ] **Step 1: Write the failing tests**

Replace the entire content of `crates/transcoderr/src/steps/probe.rs` with this version (which adds tests but stubs `metadata_path_for` so they fail to compile or panic):

```rust
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
pub(crate) fn metadata_path_for(input: &str, _ctx: &Context) -> String {
    todo!("implemented in Step 3")
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
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p transcoderr --lib metadata_path_for 2>&1 | tail -15`
Expected: 3 tests panic with `not yet implemented: implemented in Step 3`.

- [ ] **Step 3: Implement `metadata_path_for`**

Replace the body of `metadata_path_for` in `crates/transcoderr/src/steps/probe.rs`:

```rust
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

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test -p transcoderr --lib metadata_path_for 2>&1 | tail -15`
Expected: 3 tests pass.

- [ ] **Step 5: Run a wider check to ensure no regressions**

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: all `ok` lines, no `FAILED` (the metrics integration test may flake; that's pre-existing).

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print: feature/iso-bluray
git add crates/transcoderr/src/steps/probe.rs
git commit -m "feat(probe): metadata_path_for falls back to replaced_input_path on bluray: URL"
```

---

## Task 2: Replace `iso.extract` body and clean up 7z code

**Files:**
- Modify: `crates/transcoderr/src/steps/iso_extract.rs`
- Delete: `crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt`

This task wholesale replaces `iso_extract.rs` content. The new file is much smaller — no parser, no subprocess, just the gate + URL rewrite + path math.

- [ ] **Step 1: Replace `iso_extract.rs` content**

Replace the entire content of `crates/transcoderr/src/steps/iso_extract.rs`:

```rust
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
```

This is a complete rewrite. The deleted code (parser `Entry`/`parse_7z_listing`/`pick_largest_m2ts`, helpers `run_7z_list`/`run_7z_extract_to`, and the 5 parser tests) all go away. The retained tests are `step_skips_non_iso_input` and `swap_extension_replaces_iso`. The new test is `step_records_bluray_url_for_iso_input`.

- [ ] **Step 2: Delete the now-orphaned fixture file**

Run from the repo root:

```bash
git rm crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt
```

(If the `tests/fixtures/` directory becomes empty, leave it — `git` doesn't track empty dirs and Cargo might still expect the path to exist. A subsequent commit can remove the dir if it's still empty.)

- [ ] **Step 3: Build and verify**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build. The compiler will flag unused-imports if any 7z imports lingered — there shouldn't be any since you replaced the whole file.

- [ ] **Step 4: Run iso_extract tests**

Run: `cargo test -p transcoderr --lib iso_extract 2>&1 | tail -15`
Expected: 3 tests pass (`step_skips_non_iso_input`, `swap_extension_replaces_iso`, `step_records_bluray_url_for_iso_input`). The 5 deleted parser tests should NOT appear in the output.

- [ ] **Step 5: Run a wider check**

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: all `ok`, no `FAILED` (metrics flake notwithstanding).

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print: feature/iso-bluray
git add crates/transcoderr/src/steps/iso_extract.rs crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt
git commit -m "feat(iso-extract): rewrite as bluray: URL via libbluray (replaces 7z)"
```

(`git add` will pick up the deletion staged by `git rm` in Step 2. The commit lists the file path even though it's deleted.)

---

## Task 3: Drop `iso.extract` from the engine timeout alternation

**Files:**
- Modify: `crates/transcoderr/src/flow/engine.rs:111`

The 24h timeout was added because 7z extraction of a 50GB ISO could legitimately take that long. The new step does string manipulation in <1ms; it falls through to the default 600s naturally.

- [ ] **Step 1: Find the line**

Run: `grep -n iso.extract crates/transcoderr/src/flow/engine.rs`
Expected: a single line, around line 111, of the form `| "iso.extract" => 86_400,`.

- [ ] **Step 2: Apply the edit**

In `crates/transcoderr/src/flow/engine.rs`, find:

```rust
                                    "plan.execute"
                                    | "transcode"
                                    | "audio.ensure"
                                    | "remux"
                                    | "strip.tracks"
                                    | "extract.subs"
                                    | "iso.extract" => 86_400,
```

Replace with:

```rust
                                    "plan.execute"
                                    | "transcode"
                                    | "audio.ensure"
                                    | "remux"
                                    | "strip.tracks"
                                    | "extract.subs" => 86_400,
```

(Just delete the `| "iso.extract"` line — the trailing `=> 86_400,` stays on the `extract.subs` line.)

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/iso-bluray
git add crates/transcoderr/src/flow/engine.rs
git commit -m "refactor(engine): drop iso.extract from long-running timeout list"
```

---

## Task 4: Remove `p7zip-full` from container Dockerfiles

**Files:**
- Modify: `docker/Dockerfile.cpu` (line 21)
- Modify: `docker/Dockerfile.intel` (line 20)
- Modify: `docker/Dockerfile.nvidia` (line 17)
- Modify: `docker/Dockerfile.full` (line 18)

Each Dockerfile installs `p7zip-full` via `apt-get install`. The package list is on a single line; just remove the `p7zip-full ` token (with its trailing space).

- [ ] **Step 1: Edit `docker/Dockerfile.cpu`**

Find:

```dockerfile
      ca-certificates ffmpeg p7zip-full tini \
```

Replace with:

```dockerfile
      ca-certificates ffmpeg tini \
```

- [ ] **Step 2: Edit `docker/Dockerfile.intel`**

Find:

```dockerfile
      ca-certificates ffmpeg p7zip-full tini \
```

Replace with:

```dockerfile
      ca-certificates ffmpeg tini \
```

- [ ] **Step 3: Edit `docker/Dockerfile.nvidia`**

Find:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates p7zip-full tini \
```

Replace with:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tini \
```

- [ ] **Step 4: Edit `docker/Dockerfile.full`**

Find:

```dockerfile
      ca-certificates p7zip-full tini \
```

Replace with:

```dockerfile
      ca-certificates tini \
```

- [ ] **Step 5: Verify all four files no longer mention p7zip**

Run from the repo root:

```bash
grep -l p7zip docker/Dockerfile.{cpu,intel,nvidia,full}
```

Expected: no output (no file matches).

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print: feature/iso-bluray
git add docker/Dockerfile.cpu docker/Dockerfile.intel docker/Dockerfile.nvidia docker/Dockerfile.full
git commit -m "build(docker): drop p7zip-full (no longer used after iso.extract pivot)"
```

---

## Task 5: Verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm branch and clean state**

Run: `git branch --show-current && git status --short`
Expected: `feature/iso-bluray`, no uncommitted changes.

- [ ] **Step 2: Run the full per-crate test suite**

Run: `cargo test -p transcoderr --locked --lib --tests 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: every line `ok`. No `FAILED` lines except possibly the pre-existing `metrics` flake (which the iso-extract feature branch's PR description already documented).

- [ ] **Step 3: Confirm the dead code is gone**

Run: `grep -E "parse_7z_listing|pick_largest_m2ts|run_7z_list|run_7z_extract_to" crates/transcoderr/src/steps/iso_extract.rs`
Expected: no output.

Run: `ls crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt 2>&1`
Expected: `No such file or directory`.

- [ ] **Step 4: Confirm Dockerfiles are clean**

Run: `grep -l p7zip docker/Dockerfile.{cpu,intel,nvidia,full}`
Expected: no output.

- [ ] **Step 5: Confirm `engine.rs` no longer lists `iso.extract`**

Run: `grep -n iso.extract crates/transcoderr/src/flow/engine.rs`
Expected: no output.

- [ ] **Step 6: Branch commit list**

Run: `git log --oneline feature/iso-bluray ^main`
Expected (in some order): the spec commit + 4 implementation commits:

```
build(docker): drop p7zip-full (no longer used after iso.extract pivot)
refactor(engine): drop iso.extract from long-running timeout list
feat(iso-extract): rewrite as bluray: URL via libbluray (replaces 7z)
feat(probe): metadata_path_for falls back to replaced_input_path on bluray: URL
docs(spec): iso via libbluray (supersedes v0.8.0 7z approach)
```

- [ ] **Step 7: Optional manual end-to-end on the dev server**

If the user's dev server is reachable: deploy the binary, drop a Blu-ray ISO into a Radarr-watched directory, watch the run reach `output:replace` cleanly, verify the original `.iso` is gone and a `.mkv` is in place. The `ffprobe -i bluray:/path/...iso` invocation that demonstrates libbluray reads the ISO has already been confirmed during brainstorming.

If not reachable, skip — per-crate tests above already verify the unit-testable surface area.

- [ ] **Step 8: (No commit — verification only.)** The branch is ready for review/merge.
