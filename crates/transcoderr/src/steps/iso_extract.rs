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
    let mut stderr = child.stderr.take().expect("stderr piped");
    let mut file = tokio::fs::File::create(output_path).await?;

    // Drain stdout (extracted bytes) and stderr (diagnostic output) concurrently
    // so a chatty 7z (e.g. a corrupted ISO emitting >64KB of warnings) can't
    // deadlock on its stderr pipe filling while we're stuck on stdout.
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });
    let copy_result = tokio::io::copy(&mut stdout, &mut file).await;

    file.flush().await?;
    let stderr_buf = stderr_task.await.unwrap_or_default();
    let status = child.wait().await?;

    copy_result?;
    if !status.success() {
        // If extraction half-completed, the partial file lives at output_path.
        // It is NOT in ctx.steps["transcode"] yet (record_output runs after this
        // function returns), so engine::cleanup_staged_tmp won't see it on
        // failure. The file path matches the .tcr-NN.tmp.m2ts pattern so an
        // operator can identify and remove it manually if needed.
        anyhow::bail!(
            "iso.extract: 7z extract exit code {:?}: {}",
            status.code(),
            stderr_buf
        );
    }
    Ok(())
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
}
