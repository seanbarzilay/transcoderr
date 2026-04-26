# ISO via libbluray — Design

**Date:** 2026-04-26
**Branch:** `feature/iso-bluray`
**Status:** Draft, pending implementation
**Supersedes:** `2026-04-26-iso-extract-design.md` (merged in v0.8.0, but the
chosen tool — `7z` — does not work on production ISOs).

## Goal

Replace the `7z`-based `iso.extract` body merged in v0.8.0 with a tiny
URL-rewrite that hands the ISO to ffmpeg via the `bluray:` protocol. The
runtime image (`jrottenberg/ffmpeg:6.0-nvidia2204`) already ships ffmpeg
with `--enable-libbluray`, so no new package installs are required.

The triggering failure is run #23: the same Blu-ray remux ISO that
motivated the v0.8.0 work fails again, this time at `iso-extract: 7z list
exit code Some(2): Can not open the file as archive`. The runtime
container's `p7zip-full` is 16.02 (Ubuntu 22.04 default) which does not
implement UDF 2.50 — the format used by virtually all modern BD remuxes.
The exact same ISO probes cleanly via `ffprobe -i bluray:/path/...iso`,
which auto-selects the main `.mpls` playlist and exposes all video,
audio, and subtitle tracks.

## Non-goals

Same as the merged spec; restated for clarity:

- DVD ISO support (`VIDEO_TS/*.VOB`). libbluray rejects DVD ISOs
  cleanly; the failure surfaces at probe.
- Encrypted commercial Blu-rays (libbluray + AACS keys).
- Multi-disc / non-default playlist selection. libbluray's
  auto-selected playlist is correct for >95% of remuxes.
- General fix for the latent `output:replace` extension-mismatch bug.
- Wiring the `match: file.size_gb` filter into the engine.

## Design

### `iso.extract` step body

The step gates on `.iso` (case-insensitive) — unchanged. Instead of
running `7z l` + `7z e -so`, the new body builds a `bluray:` URL and
records it in the staging chain:

```rust
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
        .unwrap_or("mkv");

    let bluray_url = format!("bluray:{input_path}");
    on_progress(StepProgress::Log(format!(
        "iso.extract: routing as Blu-ray URL: {bluray_url}"
    )));

    // Put the URL into the staging chain head. Downstream steps that
    // pass it to ffmpeg's `-i` get it as-is; ffmpeg + libbluray handle
    // the bluray: protocol natively.
    staging::record_output(ctx, std::path::Path::new(&bluray_url), json!({}));

    // Record the original ISO path for output:replace to delete on
    // success (same mechanism as v0.8.0).
    ctx.steps.insert(
        "iso_extract".into(),
        json!({"replaced_input_path": &input_path}),
    );

    // Mutate ctx.file.path to the intended final basename. The .mkv
    // doesn't exist on disk yet — output:replace renames the transcoded
    // tmp onto it at the end of the flow.
    let new_path = swap_extension(&input_path, target_extension);
    on_progress(StepProgress::Log(format!(
        "iso.extract: ctx.file.path {input_path} -> {new_path}"
    )));
    ctx.file.path = new_path;

    Ok(())
}
```

The `swap_extension` helper from v0.8.0 stays untouched. The step takes
sub-millisecond time; no subprocess invocation.

### Probe `size_bytes` fallback

`crates/transcoderr/src/steps/probe.rs` is updated to handle URL chain
heads. Today's body:

```rust
let input = staging::current_input(ctx).to_string();
let path = Path::new(&input);
on_progress(StepProgress::Log(format!("probing {}", path.display())));
let v = ffprobe_json(path).await?;
ctx.probe = Some(v);
let meta = std::fs::metadata(path)?;
ctx.file.size_bytes = Some(meta.len());
Ok(())
```

becomes:

```rust
let input = staging::current_input(ctx).to_string();
let path = Path::new(&input);
on_progress(StepProgress::Log(format!("probing {}", path.display())));
let v = ffprobe_json(path).await?;
ctx.probe = Some(v);

// fs::metadata can't read a `bluray:` URL. When the chain head is one,
// fall back to the original on-disk ISO path that iso.extract recorded
// — that's the file the user actually has, and "size_bytes" is most
// intuitive when it reports that.
let metadata_path = if input.starts_with("bluray:") {
    ctx.steps
        .get("iso_extract")
        .and_then(|s| s.get("replaced_input_path"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(input.clone())
} else {
    input.clone()
};
let meta = std::fs::metadata(Path::new(&metadata_path))?;
ctx.file.size_bytes = Some(meta.len());
Ok(())
```

`ffprobe_json` already accepts `bluray:` URLs (verified directly on the
production container). Only the `fs::metadata` call needs the branch.

### Cleanup of v0.8.0 cruft

Code removed from `crates/transcoderr/src/steps/iso_extract.rs`:

- `pub struct Entry`, `parse_7z_listing`, `pick_largest_m2ts`
- `run_7z_list`, `run_7z_extract_to`
- The five 7z-listing parser tests
  (`parses_entries_from_real_listing`, `picks_largest_m2ts`,
  `picks_none_when_no_bdmv_streams`,
  `ignores_folder_entries_with_matching_path`, `case_insensitive_match`)

File removed:

- `crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt`

Tests retained and one added:

- `step_skips_non_iso_input` (gate behavior — unchanged)
- `swap_extension_replaces_iso` (path math — unchanged)
- New: `step_records_bluray_url_for_iso_input` — runs the step on an
  `.iso` input, asserts `staging::current_input(ctx)` is the
  `bluray:`-prefixed URL, `ctx.steps["iso_extract"]["replaced_input_path"]`
  is the original path, `ctx.file.path` is the swapped basename.

Engine timeout removed:

- `crates/transcoderr/src/flow/engine.rs` no longer lists `iso.extract`
  in the 86,400-second alternation. The step does string manipulation
  in <1ms; the default 600-second timeout is generous.

Dockerfile cleanup:

- `p7zip-full` removed from the `apt-get install` block in each of
  `docker/Dockerfile.{cpu,intel,nvidia,full}`. Saves ~3MB per image and
  signals that the runtime image's only required external tool is
  ffmpeg. (16.02 wouldn't help anyway; if 7z is genuinely needed in
  future for some other purpose, a working version would have to be
  installed deliberately.)

## Testing

Three unit tests in `crates/transcoderr/src/steps/iso_extract.rs`
(reduced from seven in v0.8.0):

1. `step_skips_non_iso_input` — runs the step with a non-`.iso` path,
   asserts no chain mutation, no `iso_extract` key, file.path
   unchanged.
2. `swap_extension_replaces_iso` — pins the `.iso → .mkv` path math.
3. `step_records_bluray_url_for_iso_input` — runs the step with a
   simulated ISO path (no real file required, no subprocess), asserts:
   - `staging::current_input(ctx) == "bluray:/path/Movie.iso"`
   - `ctx.steps["iso_extract"]["replaced_input_path"] == "/path/Movie.iso"`
   - `ctx.file.path == "/path/Movie.mkv"`

End-to-end (real Blu-ray ISO → ffmpeg via `bluray:` → transcoded `.mkv`)
is left to manual verification on the user's dev server. The
`ffprobe -i bluray:/path/...iso` invocation that demonstrates this
works has already been performed during brainstorming.

## Acceptance

The branch is ready to merge when:

- The new `iso.extract` body runs without invoking 7z.
- `probe` reports `size_bytes` as the original ISO's file size when the
  chain head is a `bluray:` URL.
- All 3 retained iso_extract tests pass.
- All 3 retained output tests still pass (no changes to `output:replace`).
- All four `docker/Dockerfile.*` files no longer install `p7zip-full`.
- `engine.rs` does not list `iso.extract` in the long-running step
  alternation.
- `cargo test -p transcoderr --locked --lib --tests` passes.
