# ISO Extraction Step — Design

**Date:** 2026-04-26
**Branch:** `feature/iso-extract`
**Status:** Draft, pending implementation

## Goal

Add a new built-in flow step `iso.extract` that detects a Blu-ray ISO
input, demuxes the largest `BDMV/STREAM/*.m2ts` to a sibling tmp file,
and threads it through the rest of the flow so the existing `probe →
plan → plan.execute → output:replace` chain can transcode it normally.

The concrete trigger is run 20 (`hevc-normalize` flow on
`Unlocked.2017.1080p.Blu-ray.Remux.AVC.DTS-HD.MA5.1-52pt.iso`), which
failed at `plan.audio.ensure: no non-commentary audio stream to seed
from`. Cause: ffprobe handed an ISO container sees only the wrapper, not
the inner BDMV/M2TS streams.

After this branch, the flow YAML adds one line (`use: iso.extract`) at
the top and the same input transcodes successfully to `Unlocked.mkv`,
with the original `.iso` deleted on success.

## Non-goals

Explicitly out of scope:

- DVD ISO support (`VIDEO_TS/*.VOB`).
- Encrypted commercial Blu-rays (libbluray + AACS keys).
- Multi-segment title selection via `BDMV/PLAYLIST/*.mpls` parsing
  ("largest M2TS by file size" is correct >95% of the time for
  remuxes; principled mpls parsing is a follow-up).
- Fully fixing the latent extension-mismatch bug in `output:replace`
  (today, a `.mp4 → .mkv` flow produces a `.mp4`-named file with MKV
  bytes). This branch only addresses the ISO case; a general fix
  belongs in its own change.

## Design

### New built-in step: `iso.extract`

New file `crates/transcoderr/src/steps/iso_extract.rs`. Registered in
`crates/transcoderr/src/steps/builtin.rs` alongside existing built-ins
(`probe`, `plan.video.encode`, etc.).

Behavior:

1. If `ctx.file.path` doesn't end with `.iso` (case-insensitive), log
   `"iso.extract: not an iso, skipping"` and return `Ok(())`.
2. Run `7z l -slt {iso}` to enumerate ISO contents.
3. Parse the `-slt` output into entries with `Path`, `Size`, and
   `Folder` fields. Filter to non-folder entries whose path matches
   `BDMV/STREAM/*.m2ts` (case-insensitive). Pick the largest by `Size`.
4. If no such entry exists → fail the flow with
   `iso.extract: not a Blu-ray ISO (no BDMV/STREAM/*.m2ts)`.
5. Allocate a sibling staged tmp via `staging::next_io(ctx, "m2ts")`,
   yielding e.g. `/movies/.../Unlocked.iso.tcr-00.tmp.m2ts`.
6. Stream-extract: `7z e -so {iso} {entry_path}` piped to the staged
   tmp file. Emit `StepProgress::Log` periodically with bytes-out so
   the run UI shows progress.
7. Record outputs in the context:
   - The staged M2TS path goes through the existing
     `staging::record_output(ctx, &output, json!({}))` helper, which
     writes it to `ctx.steps["transcode"]["output_path"]` (the
     codebase's single staging-chain key — historical name, treated
     as generic). This makes the M2TS the chain head so subsequent
     transformer steps and the updated `probe` see it.
   - The original ISO path goes into `ctx.steps["iso_extract"]["replaced_input_path"]`
     as its own dedicated key (NOT inside `transcode`, because
     `staging::record_output` overwrites the `transcode` map on
     subsequent chain steps and would wipe it). Consumed by
     `output:replace`.
8. Mutate `ctx.file.path` from `Unlocked.iso` to `Unlocked.{ext}` where
   `{ext}` comes from the step's `with.target_extension` YAML
   parameter, defaulting to `"mkv"`. (Note: `iso.extract` runs *before*
   `plan.init`, so the planned container isn't available in the
   context yet; the flow author specifies the intended final extension
   inline. For the only flow that ships today — `hevc-normalize` — the
   default `mkv` is correct, so the YAML doesn't need to set it
   explicitly.) `Unlocked.mkv` doesn't exist on disk at this point;
   it's the intended *final* name that `output:replace` will rename onto.

Failure modes (each emits a `kind: "failed"` event with the error
string):
- 7z not on PATH → `iso.extract: 7z not found on PATH (install p7zip-full / p7zip)`
- ISO has no BD streams → `iso.extract: not a Blu-ray ISO (no BDMV/STREAM/*.m2ts)`
- Disk full mid-extract → `iso.extract: extraction failed: <io error>`
- ISO9660/UDF parse error from 7z → `iso.extract: 7z exit code <n>: <stderr>`

### Probe staging-chain awareness

Today `probe` reads `ctx.file.path` directly to feed ffprobe. After
this change, `probe` reads from the staging chain (the same fallback
the transformer steps already use):

```rust
fn current_input(ctx: &Context) -> &str {
    ctx.steps.get("transcode")
        .and_then(|s| s.get("output_path"))
        .and_then(|v| v.as_str())
        .unwrap_or(&ctx.file.path)
}
```

Lives as `staging::current_input(ctx)` next to the existing
`staging::next_io` and `staging::record_output`. One-line change in
`probe.rs` to use it instead of reading `ctx.file.path` directly. The
existing transformer steps already have this fallback baked into
`staging::next_io` so they need no change.

### `output:replace` deletes the replaced input on success

`crates/transcoderr/src/steps/output.rs` currently does only:

```rust
std::fs::rename(staged, ctx.file.path)
```

After this change:

```rust
let original = ctx.file.path.clone();
std::fs::rename(&staged, &original)?;
if let Some(replaced) = ctx.steps.get("iso_extract")
    .and_then(|s| s.get("replaced_input_path"))
    .and_then(|v| v.as_str())
{
    if let Err(e) = std::fs::remove_file(replaced) {
        on_progress(StepProgress::Log(format!(
            "warn: failed to delete replaced input {replaced}: {e}"
        )));
    }
}
Ok(())
```

Best-effort delete: if the .iso can't be removed (permission, race) we
log and continue — the .mkv is already in the user's library, so the
operation succeeded in the meaningful sense.

When `iso.extract` ran, `ctx.file.path` is `Unlocked.mkv`, the staged
tmp is the transcoded `.mkv`, and `replaced_input_path` is the original
`Unlocked.iso`. Atomic rename lands the .mkv at the right name; the .iso
is then deleted. When `iso.extract` did NOT run (or no-op'd because the
input wasn't an ISO), the new code path is skipped and behavior is
identical to today.

Order of operations is intentional: rename first (atomic, the new file
is in place), then delete the .iso. If the delete fails for some reason
(permission, race), the .mkv is already in the user's library and the
.iso lingers — manual cleanup, but no data loss.

### Dockerfile additions

Add `p7zip-full` to the runtime stage's `apt-get install` list in each
of the four `docker/Dockerfile.*` files. The four runtime bases are:

- `Dockerfile.cpu`, `Dockerfile.intel`: `debian:bookworm-slim`
- `Dockerfile.nvidia`, `Dockerfile.full`: `jrottenberg/ffmpeg:6.0-nvidia2204` (Ubuntu 22.04)

`p7zip-full` is the same package name on both bases. Adds ~3MB.

For local development (`cargo run -p transcoderr`), the developer needs
`7z` on PATH (`brew install p7zip` on macOS, `apt-get install p7zip-full`
on Debian/Ubuntu). The README's dev-setup section is not updated by
this branch — the failure mode is the clear "7z not found on PATH"
error from step 1.

### Flow YAML change

`docs/flows/hevc-normalize.yaml` gains one line at the top of the
`steps:` list:

```yaml
- id: iso-extract
  use: iso.extract
```

No other flow YAML changes. The step's internal gate (skip if not an
ISO) means flows always running this step are correct for any input.

## Testing

Three test layers, no real ISO fixtures needed:

1. **Parser unit tests.** A pure function `parse_7z_listing(stdout: &str) -> Vec<Entry>`
   and `pick_largest_m2ts(entries: &[Entry]) -> Option<&Entry>` in
   `iso_extract.rs`. Tests feed canned `7z l -slt` output captured once
   from a real Blu-ray ISO (committed to
   `crates/transcoderr/tests/fixtures/7z_listing_bdmv.txt`) and assert
   the right entry is picked.

2. **No-op gate test.** `iso.extract` step run with a non-`.iso`
   `ctx.file.path` returns `Ok(())`, logs "skipping", and does not
   invoke 7z. Verified via dependency injection: the step takes a
   `dyn Subprocess` trait object that the test stubs.

3. **`output:replace` deletes replaced input.** Integration test in
   `crates/transcoderr/src/steps/output.rs`'s test module: create two
   tempfiles (one as the staged tmp, one as the simulated "iso path"
   stored in `replaced_input_path`), run `output:replace` with `ctx.file.path`
   pointing at a third path, assert the staged file moved to that path
   and the simulated iso path was deleted.

End-to-end (real ISO → transcoded .mkv) is left to manual verification
on the user's dev server. Would need a synthetic BD-shaped ISO fixture
(~50–100MB) to automate; explicitly out of scope.

## Acceptance

The branch is ready to merge when:

- `iso.extract` step is registered as a built-in and the `hevc-normalize`
  flow YAML invokes it.
- The three test layers above pass.
- `LOG_FORMAT=json cargo run -p transcoderr -- serve` followed by a
  webhook-triggered transcode of an ISO Blu-ray remux produces a
  `.mkv` next to where the `.iso` lived; the `.iso` is gone after the
  flow completes; the `.tcr-NN.tmp.m2ts` is cleaned up.
- All four `docker/Dockerfile.*` files install `p7zip-full`.
- `cargo test -p transcoderr --locked --lib --tests` passes.
