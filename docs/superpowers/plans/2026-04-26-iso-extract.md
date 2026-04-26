# ISO Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new built-in flow step `iso.extract` that demuxes the largest `BDMV/STREAM/*.m2ts` from a Blu-ray ISO input, threads it through the existing staging chain so the rest of the flow probes/transcodes normally, and signals `output:replace` to delete the original `.iso` after a successful transcode.

**Architecture:** The step gates on `.iso` extension, runs `7z l -slt` to enumerate the ISO, picks the largest `BDMV/STREAM/*.m2ts` by size, stream-extracts it with `7z e -so` to a sibling `.tcr-NN.tmp.m2ts`, records that path in the staging chain (`ctx.steps["transcode"]["output_path"]`), records the original ISO path in `ctx.steps["iso_extract"]["replaced_input_path"]`, and mutates `ctx.file.path` to the intended final `.mkv` name. `probe` is taught to honor the staging chain. `output:replace` deletes `replaced_input_path` after the atomic rename.

**Tech Stack:** Rust workspace (`crates/transcoderr`), async-trait Step interface, `tokio::process` for subprocess, `7z` CLI from `p7zip-full`.

**Spec:** `docs/superpowers/specs/2026-04-26-iso-extract-design.md`

---

## File Structure

```
crates/transcoderr/src/flow/staging.rs                            [modify: add current_input helper + test]
crates/transcoderr/src/steps/probe.rs                             [modify: use current_input]
crates/transcoderr/src/steps/iso_extract.rs                       [create: parser + step impl + tests]
crates/transcoderr/src/steps/mod.rs                               [modify: pub mod iso_extract]
crates/transcoderr/src/steps/builtin.rs                           [modify: register "iso.extract"]
crates/transcoderr/src/steps/output.rs                            [modify: delete replaced_input_path + test]
crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt             [create: parser fixture]
docs/flows/hevc-normalize.yaml                                    [modify: add iso-extract step at top]
docker/Dockerfile.cpu                                             [modify: install p7zip-full]
docker/Dockerfile.intel                                           [modify: install p7zip-full]
docker/Dockerfile.nvidia                                          [modify: install p7zip-full]
docker/Dockerfile.full                                            [modify: install p7zip-full]
```

---

## Task 1: `staging::current_input` helper

**Files:**
- Modify: `crates/transcoderr/src/flow/staging.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `mod tests { ... }` block at the bottom of `crates/transcoderr/src/flow/staging.rs`:

```rust
    #[test]
    fn current_input_returns_file_path_when_no_chain() {
        let ctx = Context::for_file("/m/Dune.mkv");
        assert_eq!(current_input(&ctx), "/m/Dune.mkv");
    }

    #[test]
    fn current_input_returns_chain_head_when_present() {
        let mut ctx = Context::for_file("/m/Dune.iso");
        record_output(&mut ctx, std::path::Path::new("/m/Dune.iso.tcr-00.tmp.m2ts"), json!({}));
        assert_eq!(current_input(&ctx), "/m/Dune.iso.tcr-00.tmp.m2ts");
    }
```

- [ ] **Step 2: Run the tests and watch them fail**

Run: `cargo test -p transcoderr --lib current_input 2>&1 | tail -10`
Expected: compile error — function `current_input` not found.

- [ ] **Step 3: Implement `current_input`**

Add to `crates/transcoderr/src/flow/staging.rs` (right before `#[cfg(test)]`):

```rust
/// The current input path for steps that consume the staging chain.
/// Returns the latest staged tmp file (chain head) if one exists, else the
/// original `ctx.file.path`. Used by `probe` so it sees what transformer
/// steps have produced upstream (e.g. an extracted M2TS from `iso.extract`).
pub fn current_input(ctx: &Context) -> &str {
    ctx.steps
        .get("transcode")
        .and_then(|v| v.get("output_path"))
        .and_then(|v| v.as_str())
        .unwrap_or(&ctx.file.path)
}
```

- [ ] **Step 4: Run the tests and watch them pass**

Run: `cargo test -p transcoderr --lib current_input 2>&1 | tail -10`
Expected: `2 passed; 0 failed`.

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/flow/staging.rs
git commit -m "feat(staging): current_input helper reads chain head or file.path"
```

---

## Task 2: Probe uses the staging chain

**Files:**
- Modify: `crates/transcoderr/src/steps/probe.rs`

- [ ] **Step 1: Replace probe.rs body**

Replace the entire content of `crates/transcoderr/src/steps/probe.rs`:

```rust
use super::{Step, StepProgress};
use crate::ffmpeg::ffprobe_json;
use crate::flow::{staging, Context};
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
        let input = staging::current_input(ctx).to_string();
        let path = Path::new(&input);
        on_progress(StepProgress::Log(format!("probing {}", path.display())));
        let v = ffprobe_json(path).await?;
        ctx.probe = Some(v);
        let meta = std::fs::metadata(path)?;
        ctx.file.size_bytes = Some(meta.len());
        Ok(())
    }
}
```

The two changes from before: import `staging`, and replace the direct `ctx.file.path` read with `staging::current_input(ctx)`. Note `current_input` returns `&str` borrowed from `ctx`, and we need the path to outlive the borrow (because `metadata` reads it after `ctx.probe = Some(v)` mutably borrows `ctx`), so we clone via `to_string()`.

- [ ] **Step 2: Build and verify**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build. The previously-passing tests should still pass; verify with:

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -20`
Expected: no FAILED lines (the `metrics` integration test may flake — see plan §verification — but the `probe.rs`-touching tests should all pass).

- [ ] **Step 3: Commit**

```bash
git add crates/transcoderr/src/steps/probe.rs
git commit -m "feat(probe): read staging chain head so iso.extract output is probed"
```

---

## Task 3: 7z listing parser (pure functions)

**Files:**
- Create: `crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt`
- Create (partial): `crates/transcoderr/src/steps/iso_extract.rs`

This task creates the new step file with ONLY the pure parser functions and their tests. The async step impl is added in Task 4.

- [ ] **Step 1: Create the fixture file**

Create `crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt` with this exact content:

```

7-Zip [64] 17.04 : Copyright (c) 1999-2017 Igor Pavlov : 2017-08-28
p7zip Version 17.04 (locale=en_US.UTF-8,Utf16=on,HugeFiles=on,64 bits,4 CPUs)

Listing archive: /movies/Unlocked.iso

--
Path = /movies/Unlocked.iso
Type = Iso
Physical Size = 36608360448

----------
Path = BDMV/index.bdmv
Folder = -
Size = 110

----------
Path = BDMV/STREAM
Folder = +
Size = 0

----------
Path = BDMV/STREAM/00000.m2ts
Folder = -
Size = 1572864

----------
Path = BDMV/STREAM/00001.m2ts
Folder = -
Size = 36507942912

----------
Path = BDMV/STREAM/00002.m2ts
Folder = -
Size = 1572864

----------
Path = CERTIFICATE/ID.BDMV
Folder = -
Size = 110
```

- [ ] **Step 2: Write the failing parser tests**

Create `crates/transcoderr/src/steps/iso_extract.rs`:

```rust
//! `iso.extract` step: detects Blu-ray ISO inputs, demuxes the largest
//! `BDMV/STREAM/*.m2ts`, and threads it through the staging chain.

#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub is_folder: bool,
}

/// Parse the output of `7z l -slt <iso>` into entries.
/// Skips the archive-level header (the first block before the first
/// `----------` separator) and yields one entry per file/folder block.
pub fn parse_7z_listing(stdout: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for chunk in stdout.split("----------").skip(1) {
        let mut path: Option<String> = None;
        let mut size: Option<u64> = None;
        let mut is_folder = false;
        for line in chunk.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("Path = ") {
                path = Some(rest.to_string());
            } else if let Some(rest) = line.strip_prefix("Size = ") {
                size = rest.parse().ok();
            } else if let Some(rest) = line.strip_prefix("Folder = ") {
                is_folder = rest == "+";
            }
        }
        if let (Some(p), Some(s)) = (path, size) {
            out.push(Entry { path: p, size: s, is_folder });
        }
    }
    out
}

/// Pick the largest non-folder `BDMV/STREAM/*.m2ts` entry. Case-insensitive.
pub fn pick_largest_m2ts(entries: &[Entry]) -> Option<&Entry> {
    entries
        .iter()
        .filter(|e| !e.is_folder)
        .filter(|e| {
            let lower = e.path.to_lowercase();
            lower.starts_with("bdmv/stream/") && lower.ends_with(".m2ts")
        })
        .max_by_key(|e| e.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/7z_listing_bdmv.txt");

    #[test]
    fn parses_entries_from_real_listing() {
        let entries = parse_7z_listing(FIXTURE);
        // 6 entries: index.bdmv, STREAM (folder), 3x m2ts, ID.BDMV
        assert_eq!(entries.len(), 6, "got {entries:#?}");
        assert!(entries.iter().any(|e| e.path == "BDMV/index.bdmv" && e.size == 110 && !e.is_folder));
        assert!(entries.iter().any(|e| e.path == "BDMV/STREAM" && e.is_folder));
        assert!(entries.iter().any(|e| e.path == "BDMV/STREAM/00001.m2ts" && e.size == 36507942912));
    }

    #[test]
    fn picks_largest_m2ts() {
        let entries = parse_7z_listing(FIXTURE);
        let pick = pick_largest_m2ts(&entries).expect("should find one");
        assert_eq!(pick.path, "BDMV/STREAM/00001.m2ts");
        assert_eq!(pick.size, 36507942912);
    }

    #[test]
    fn picks_none_when_no_bdmv_streams() {
        let entries = vec![
            Entry { path: "VIDEO_TS/VTS_01_1.VOB".into(), size: 1024, is_folder: false },
            Entry { path: "BDMV/STREAM".into(), size: 0, is_folder: true },
        ];
        assert!(pick_largest_m2ts(&entries).is_none());
    }

    #[test]
    fn ignores_folder_entries_with_matching_path() {
        let entries = vec![
            Entry { path: "BDMV/STREAM/junk".into(), size: 999_999_999, is_folder: true },
            Entry { path: "BDMV/STREAM/00001.m2ts".into(), size: 1024, is_folder: false },
        ];
        let pick = pick_largest_m2ts(&entries).unwrap();
        assert_eq!(pick.path, "BDMV/STREAM/00001.m2ts");
    }

    #[test]
    fn case_insensitive_match() {
        let entries = vec![
            Entry { path: "bdmv/stream/00001.M2TS".into(), size: 1024, is_folder: false },
        ];
        let pick = pick_largest_m2ts(&entries).unwrap();
        assert_eq!(pick.size, 1024);
    }
}
```

- [ ] **Step 3: Wire the new module into `mod.rs`**

In `crates/transcoderr/src/steps/mod.rs`, add `pub mod iso_extract;` between `pub mod extract_subs;` and `pub mod move_step;` (alphabetical with the existing list).

- [ ] **Step 4: Run the tests and watch them fail**

Run: `cargo test -p transcoderr --lib iso_extract 2>&1 | tail -15`
Expected: actually all 5 tests pass on the first run because the parser is fully implemented. (TDD purists: the "failing" state is the absence of the file. Once we write it, tests should pass.) If anything fails, fix the parser before continuing.

- [ ] **Step 5: Run a wider check to ensure no regressions**

Run: `cargo test -p transcoderr --lib 2>&1 | grep -E "^test result" | tail -3`
Expected: all `ok` lines, no `FAILED`.

- [ ] **Step 6: Commit**

```bash
git add crates/transcoderr/src/steps/iso_extract.rs crates/transcoderr/src/steps/mod.rs crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt
git commit -m "feat(iso-extract): pure parser for 7z -slt listings"
```

---

## Task 4: `iso.extract` step orchestration

**Files:**
- Modify: `crates/transcoderr/src/steps/iso_extract.rs`

This task adds the async `Step` impl that gates on `.iso` extension, invokes 7z, integrates with the staging chain, and mutates `ctx.file.path`.

- [ ] **Step 1: Add the step impl to `iso_extract.rs`**

Append to `crates/transcoderr/src/steps/iso_extract.rs` (after the existing pure functions, before the `mod tests` block):

```rust
use crate::flow::{staging, Context};
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

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
        // Gate: skip silently if the input isn't an ISO.
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

        // 1. List the ISO contents.
        on_progress(StepProgress::Log(format!("iso.extract: listing {input_path}")));
        let listing = run_7z_list(&input_path).await?;
        let entries = parse_7z_listing(&listing);

        // 2. Pick the largest BDMV/STREAM/*.m2ts.
        let pick = pick_largest_m2ts(&entries).ok_or_else(|| {
            anyhow::anyhow!("iso.extract: not a Blu-ray ISO (no BDMV/STREAM/*.m2ts)")
        })?;
        let pick_path = pick.path.clone();
        let pick_size = pick.size;
        on_progress(StepProgress::Log(format!(
            "iso.extract: selected {pick_path} ({pick_size} bytes)"
        )));

        // 3. Allocate a sibling staged tmp via the existing staging machinery.
        let (_, output_path) = staging::next_io(ctx, "m2ts");

        // 4. Stream-extract the chosen entry to the staged tmp.
        run_7z_extract_to(&input_path, &pick_path, &output_path, on_progress).await?;

        // 5. Record the staged output in the chain (so probe + downstream see it).
        staging::record_output(ctx, &output_path, json!({}));

        // 6. Record the original ISO path in its own key for output:replace to consume.
        //    Lives in ctx.steps["iso_extract"], NOT ctx.steps["transcode"], because
        //    staging::record_output overwrites the latter on each chain step.
        ctx.steps.insert(
            "iso_extract".into(),
            json!({"replaced_input_path": &input_path}),
        );

        // 7. Mutate ctx.file.path to the intended final basename. The file does not
        //    exist on disk yet — output:replace will atomically rename the transcoded
        //    .mkv tmp onto this path at the end of the flow.
        let new_path = swap_extension(&input_path, &target_extension);
        on_progress(StepProgress::Log(format!(
            "iso.extract: ctx.file.path {input_path} -> {new_path}"
        )));
        ctx.file.path = new_path;

        Ok(())
    }
}

/// Replace the final `.iso` (case-insensitive) on `path` with the given extension.
fn swap_extension(path: &str, new_ext: &str) -> String {
    let pb = PathBuf::from(path);
    let parent = pb.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let stem = pb.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    parent.join(format!("{stem}.{new_ext}")).to_string_lossy().into_owned()
}

async fn run_7z_list(iso_path: &str) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("7z")
        .arg("l").arg("-slt").arg(iso_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!(
                "iso.extract: 7z not found on PATH (install p7zip-full / p7zip)"
            ),
            _ => anyhow::anyhow!("iso.extract: failed to spawn 7z: {e}"),
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!(
            "iso.extract: 7z list exit code {:?}: {}",
            out.status.code(),
            stderr
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn run_7z_extract_to(
    iso_path: &str,
    entry_path: &str,
    output_path: &std::path::Path,
    on_progress: &mut (dyn FnMut(StepProgress) + Send),
) -> anyhow::Result<()> {
    on_progress(StepProgress::Log(format!(
        "iso.extract: extracting {entry_path} -> {}",
        output_path.display()
    )));
    let mut child = tokio::process::Command::new("7z")
        .arg("e").arg("-so").arg(iso_path).arg(entry_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!(
                "iso.extract: 7z not found on PATH (install p7zip-full / p7zip)"
            ),
            _ => anyhow::anyhow!("iso.extract: failed to spawn 7z: {e}"),
        })?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut file = tokio::fs::File::create(output_path).await?;
    tokio::io::copy(&mut stdout, &mut file).await?;
    file.flush().await?;
    let status = child.wait().await?;
    if !status.success() {
        let mut stderr_buf = String::new();
        if let Some(mut err) = child.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = err.read_to_string(&mut stderr_buf).await;
        }
        // If extraction half-completed, leave the partial file for the engine's
        // cleanup_staged_tmp to remove on flow failure.
        anyhow::bail!(
            "iso.extract: 7z extract exit code {:?}: {}",
            status.code(),
            stderr_buf
        );
    }
    Ok(())
}
```

Note: the `run_7z_*` helpers are not exposed as a trait — they call `tokio::process::Command` directly, matching the codebase's existing pattern (`crates/transcoderr/src/ffmpeg.rs` does the same for ffmpeg). Tests for the orchestration would require dependency injection, which isn't worth the complexity for one binary call. The gating logic and parser are tested directly; the subprocess invocation is verified by manual end-to-end run on the user's dev server (Task 9).

- [ ] **Step 2: Add a no-op gate test**

Append inside the existing `mod tests { ... }` block in `iso_extract.rs`:

```rust
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
        // The step returned Ok with one log line and didn't touch the staging chain.
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
```

The gate test executes the full step — but because the input doesn't end in `.iso`, the step returns at the gate before invoking 7z, so 7z isn't required to be on PATH for this test. The `swap_extension` test pins the path math.

- [ ] **Step 3: Build and run all the new tests**

Run: `cargo test -p transcoderr --lib iso_extract 2>&1 | tail -20`
Expected: 7 tests pass total (5 from Task 3 + 2 added here).

- [ ] **Step 4: Commit**

```bash
git add crates/transcoderr/src/steps/iso_extract.rs
git commit -m "feat(iso-extract): step impl with gate, 7z list/extract, staging integration"
```

---

## Task 5: Register `iso.extract` as a built-in

**Files:**
- Modify: `crates/transcoderr/src/steps/builtin.rs`

- [ ] **Step 1: Add the import**

In `crates/transcoderr/src/steps/builtin.rs`, find the `use crate::steps::{` block at the top (lines 2-22). Insert `iso_extract::IsoExtractStep,` between the existing `extract_subs::ExtractSubsStep,` and `move_step::MoveStep,` lines:

```rust
use crate::steps::{
    audio_ensure::AudioEnsureStep,
    copy_step::CopyStep,
    delete_step::DeleteStep,
    extract_subs::ExtractSubsStep,
    iso_extract::IsoExtractStep,
    move_step::MoveStep,
    notify::NotifyStep,
    output::OutputStep,
    plan_execute::PlanExecuteStep,
    plan_steps::{
        PlanAudioEnsureStep, PlanContainerStep, PlanDropCoverArtStep, PlanDropDataStep,
        PlanDropUnsupportedSubsStep, PlanInitStep, PlanTolerateErrorsStep, PlanVideoEncodeStep,
    },
    probe::ProbeStep,
    remux::RemuxStep,
    shell::ShellStep,
    strip_tracks::StripTracksStep,
    transcode::TranscodeStep,
    verify_playable::VerifyPlayableStep,
    Step,
};
```

- [ ] **Step 2: Register the step**

In the same file, in `register_all`, add the registration line right before `map.insert("plan.init"...)`:

```rust
    map.insert("iso.extract".into(), Arc::new(IsoExtractStep));
```

- [ ] **Step 3: Build and verify**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add crates/transcoderr/src/steps/builtin.rs
git commit -m "feat(iso-extract): register iso.extract as a built-in step"
```

---

## Task 6: `output:replace` deletes `replaced_input_path`

**Files:**
- Modify: `crates/transcoderr/src/steps/output.rs`

- [ ] **Step 1: Replace `output.rs`**

Replace the entire content of `crates/transcoderr/src/steps/output.rs`:

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p transcoderr --lib output 2>&1 | tail -15`
Expected: 3 tests pass (`replace_renames_staged_to_original`, `replace_deletes_replaced_input_when_iso_extract_ran`, `replace_skips_iso_delete_when_not_set`).

- [ ] **Step 3: Commit**

```bash
git add crates/transcoderr/src/steps/output.rs
git commit -m "feat(output): delete replaced_input_path when iso.extract ran"
```

---

## Task 7: Add `iso-extract` to the `hevc-normalize` flow YAML

**Files:**
- Modify: `docs/flows/hevc-normalize.yaml`

- [ ] **Step 1: Insert the iso-extract step at the top of `steps:`**

In `docs/flows/hevc-normalize.yaml`, find the lines:

```yaml
steps:
  - id: probe
    use: probe
```

Insert immediately after `steps:` (and before `- id: probe`):

```yaml
  # Demux the largest BDMV/STREAM/*.m2ts out of any .iso input. No-ops for
  # non-ISO inputs. Threads the m2ts through the staging chain so probe sees
  # it; output:replace deletes the original .iso on success.
  - id: iso-extract
    use: iso.extract

```

The `with: { target_extension: mkv }` is omitted because `mkv` is the default and matches what `plan.init` seeds.

- [ ] **Step 2: Verify YAML still parses**

Run: `cargo test -p transcoderr --lib --tests --locked flow 2>&1 | grep -E "^test result|FAILED" | tail -10`
Expected: all `ok`. Any flow-parsing test that loads the bundled YAMLs will exercise this.

- [ ] **Step 3: Commit**

```bash
git add docs/flows/hevc-normalize.yaml
git commit -m "feat(flows): hevc-normalize handles ISO inputs via iso.extract"
```

---

## Task 8: Install `p7zip-full` in container images

**Files:**
- Modify: `docker/Dockerfile.cpu`, `docker/Dockerfile.intel`, `docker/Dockerfile.nvidia`, `docker/Dockerfile.full`

- [ ] **Step 1: Find the runtime stage's `apt-get install` line in each file**

Each Dockerfile has a runtime stage (`FROM debian:bookworm-slim AS runtime` or `FROM jrottenberg/ffmpeg:6.0-nvidia2204 AS runtime`) followed by an `apt-get install` for runtime deps. The `LOG_FORMAT=json` line landed in `feature/structured-logging`; this branch is `feature/iso-extract` (off `main` at v0.7.1), so that line is NOT yet present in `main`.

In each of the four files, find the existing runtime-stage `RUN apt-get update && apt-get install -y --no-install-recommends \` block, and add `p7zip-full` to the package list (alphabetical). For example, in `docker/Dockerfile.cpu`:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
      ffmpeg \
      p7zip-full \
      tini \
   && rm -rf /var/lib/apt/lists/*
```

Adjust the existing list in each file (the package set differs per flavor). The principle is identical: add `p7zip-full` to whatever apt-get install command runs in the runtime stage.

- [ ] **Step 2: Verify all four files have the package**

Run: `grep -l p7zip-full docker/Dockerfile.{cpu,intel,nvidia,full}`
Expected: all four file paths printed.

- [ ] **Step 3: Commit**

```bash
git add docker/Dockerfile.cpu docker/Dockerfile.intel docker/Dockerfile.nvidia docker/Dockerfile.full
git commit -m "build(docker): install p7zip-full for iso.extract step"
```

---

## Task 9: Verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm branch and clean state**

Run: `git branch --show-current && git status --short`
Expected: `feature/iso-extract`, no uncommitted changes.

- [ ] **Step 2: Run the full per-crate test suite**

Run: `cargo test -p transcoderr --locked --lib --tests 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: every line `ok`; no `FAILED`. (The workspace-wide flake in `crates/transcoderr/tests/metrics.rs` only triggers under `cargo test --workspace`, not here.)

- [ ] **Step 3: Confirm the parser fixture is committed and discoverable**

Run: `cat crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt | head -5`
Expected: matches the fixture content from Task 3 Step 1.

- [ ] **Step 4: Manual end-to-end verification on the user's dev server**

This is the only path that exercises real 7z + a real ISO. Skip if the dev server isn't reachable; the per-crate tests above cover everything that can be verified hermetically.

If reachable, on the dev server:

1. Build and deploy the binary (`cargo build --release -p transcoderr` and copy to the server, or `cargo run`).
2. Drop a small Blu-ray ISO at a path Radarr is configured to scan, or trigger a webhook directly via the HTTP API.
3. Watch the run: it should progress through `iso-extract` → `probe` → `plan-*` → `plan-execute` → `verify-playable` → `output`.
4. After completion: the original `.iso` is gone; a new `.mkv` exists at the same basename; the `.tcr-NN.tmp.m2ts` file is also gone.

If any of those don't hold, escalate.

- [ ] **Step 5: Branch commit list**

Run: `git log --oneline feature/iso-extract ^main`
Expected (8 commits from this work):

```
build(docker): install p7zip-full for iso.extract step
feat(flows): hevc-normalize handles ISO inputs via iso.extract
feat(output): delete replaced_input_path when iso.extract ran
feat(iso-extract): register iso.extract as a built-in step
feat(iso-extract): step impl with gate, 7z list/extract, staging integration
feat(iso-extract): pure parser for 7z -slt listings
feat(probe): read staging chain head so iso.extract output is probed
feat(staging): current_input helper reads chain head or file.path
docs(spec): align iso-extract spec with codebase staging convention
docs(spec): iso extraction step design
```

(plus any prior spec/plan commits already on the branch).

- [ ] **Step 6: (No commit — verification only.)** The branch is ready for review/merge.
