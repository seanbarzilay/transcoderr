# Container-aware `output:replace` — Design

**Date:** 2026-04-27
**Branch:** `feature/output-container-aware`
**Status:** Draft, pending implementation
**Builds on:** v0.8.2 (commit `e7d566f`)

## Goal

Fix the latent extension-mismatch bug in `output:replace`: today the
step does `std::fs::rename(staged, ctx.file.path)` which preserves
the source's extension, so an `.mp4 → .mkv` flow produces an `.mp4`-
named file containing MKV bytes. The user-visible bug is "the output
filename should match the planned container".

The v0.8.1 iso work already solved this for ISO inputs in a peculiar
way — `iso.extract` mutates `ctx.file.path` from `Movie.iso` to
`Movie.mkv` and records `ctx.steps["iso_extract"]["replaced_input_path"]`
for `output:replace` to consume. That's an iso-specific hack.

This branch unifies: `output:replace` becomes container-aware. It
reads the planned container from `ctx.steps["_plan"]["container"]`,
computes the desired final path, atomic-renames the staged tmp to
that path, and best-effort-deletes the source if the extension
changed. The iso-specific path mutation and `replaced_input_path`
indirection both go away. `ctx.file.path` stays as "the user's
original input" throughout the flow.

## Non-goals

- New `mode:` values for `output` (e.g. `mode: alongside` to keep the
  source). The step's verb is "replace"; if anyone needs alongside-
  semantics later, it gets its own mode.
- Per-container subtitle compatibility tables. `SUPPORTED_SUB_CODECS`
  remains an implicitly-MKV list until other output containers ship.
- Migration of pre-existing in-flight checkpoints. The
  `iso_extract.replaced_input_path` key in persisted snapshots
  becomes dead JSON; the new code ignores it. No migration needed.
- A new template variable exposing the destination path to the
  notify step. (See "Notify template behavior" under Out of scope.)
- Backward-compat for flows that don't run `plan.execute`. They keep
  today's verbatim-rename behavior; the new logic is gated on a plan
  being present.

## Design

### `output:replace` becomes container-aware

`crates/transcoderr/src/steps/output.rs` is rewritten:

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
        // and the .mp4 is deleted. Same-extension flows (mkv → mkv)
        // keep today's atomic in-place rename. No plan → no extension
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
        // delete failure is non-fatal — we log and continue.
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
```

Key points:

- `swap_extension` moves here from `iso_extract.rs` (its only caller
  after the refactor; the function is private to `output.rs`).
- The `iso_extract.replaced_input_path` lookup is gone entirely —
  the deletion target is now derived from the path comparison
  `final_path != original`, which covers both ISO and non-ISO cases.
- "No plan in context" → no extension swap, no source delete. Same
  behavior as today for any flow that doesn't go through
  `plan.execute`.

### `iso.extract` simplifies

`crates/transcoderr/src/steps/iso_extract.rs` body shrinks to gate +
URL rewrite. No `ctx.file.path` mutation, no `replaced_input_path`
recording, no `with.target_extension` parameter:

```rust
//! `iso.extract` step: detects Blu-ray ISO inputs and rewrites the
//! staging chain head to a `bluray:` URL so ffmpeg (with libbluray)
//! can ingest the disc directly. No on-disk extraction; the step is
//! pure string manipulation and takes <1ms.
//!
//! The output filename's extension is decided later, by
//! `output:replace`, based on the plan's `container` field.
//! iso.extract no longer touches `ctx.file.path` — the user's
//! original input path stays stable throughout the flow.

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
```

Removed code:

- `swap_extension` helper (moved to `output.rs`).
- `with.target_extension` parameter handling.
- `ctx.file.path` mutation (the `let new_path = swap_extension(...);
  ctx.file.path = new_path;` block).
- `ctx.steps.insert("iso_extract", json!({"replaced_input_path": ...}))`.

### `probe::metadata_path_for` strips the protocol prefix

`crates/transcoderr/src/steps/probe.rs::metadata_path_for` today
falls back to `ctx.steps["iso_extract"]["replaced_input_path"]` for
`bluray:` URLs. After this refactor that key never gets set. New
behavior: just strip the 7-char `"bluray:"` prefix.

```rust
/// Resolve the path that should be passed to `fs::metadata`. For a
/// `bluray:` URL chain head, strip the protocol prefix to recover
/// the underlying ISO path. For any other input, return it
/// unchanged.
pub(crate) fn metadata_path_for(input: &str, _ctx: &Context) -> String {
    if let Some(real) = input.strip_prefix("bluray:") {
        real.to_string()
    } else {
        input.to_string()
    }
}
```

The `_ctx: &Context` parameter is kept (callers still pass it) so
the public signature stays stable, but the body no longer reads from
it.

## Testing

Tests are reorganized to match the new mechanism:

**`output.rs`** ends up with 4 tests, all hermetic (use
`tempfile::tempdir`, no subprocess):

1. `replace_in_place_when_extensions_match` — source `Movie.mkv` +
   plan `container=mkv` → atomic rename overwrites `Movie.mkv`. No
   second delete. The "no behavior change for same-extension flows"
   assertion.
2. `replace_swaps_extension_and_deletes_source` — source `Movie.mp4`
   + plan `container=mkv` → `Movie.mkv` lands, `Movie.mp4` deleted.
   The new mechanism in action.
3. `replace_no_plan_falls_back_to_in_place_rename` — no `_plan` key
   in `ctx.steps` → today's verbatim rename, no source delete. The
   backward-compat path.
4. `replace_renames_staged_to_original` — kept (slightly tweaked: the
   `_plan` key gets seeded with `container: "mkv"` matching the
   source's `.mkv` extension). Asserts staged content lands at the
   original path.

The two iso-flavored tests in today's `output.rs` go away
(`replace_deletes_replaced_input_when_iso_extract_ran` and
`replace_skips_iso_delete_when_not_set`) — the mechanism they
exercised no longer exists.

**`iso_extract.rs`** ends up with 2 tests:

1. `step_skips_non_iso_input` — kept (no longer asserts about
   `iso_extract` key absence beyond what's natural).
2. `step_records_bluray_url_for_iso_input` — rewritten. Asserts:
   - `staging::current_input(&ctx) == "bluray:/movies/Unlocked.iso"`
   - `ctx.file.path == "/movies/Unlocked.iso"` (UNCHANGED, the new
     invariant)
   - `ctx.steps.get("iso_extract").is_none()` (the key is no longer
     written)

The `swap_extension_replaces_iso` test goes away (the helper moved
to `output.rs` and gets its own test there if needed; the path math
is identical so a separate test isn't required).

**`probe.rs`** ends up with 2 tests:

1. `metadata_path_for_plain_path_returns_input` — unchanged.
2. `metadata_path_for_bluray_url_strips_protocol_prefix` — renamed
   from `metadata_path_for_bluray_url_falls_back_to_replaced_input`.
   Asserts `bluray:/movies/Unlocked.iso → /movies/Unlocked.iso`. No
   longer needs to seed `ctx.steps["iso_extract"]`.

The third probe test
(`metadata_path_for_bluray_url_with_no_replaced_input_returns_url_as_string`)
is **deleted** — the "edge case" it covered (URL but no iso_extract
key) is now the normal path; the prefix strip handles it directly.

End-to-end (real ISO + real MP4 → real container change → real
delete) is left to manual verification on the dev server.

## Acceptance

The branch is ready to merge when:

- `output.rs` reads `plan.container`, swaps the source's extension to
  it for the rename target, and best-effort-deletes the source if the
  extension changed.
- `iso_extract.rs` does NOT mutate `ctx.file.path` and does NOT
  write `ctx.steps["iso_extract"]`. Its execute body is just gate +
  URL rewrite.
- `probe.rs::metadata_path_for` strips the `bluray:` prefix instead
  of consulting `ctx.steps["iso_extract"]`.
- All 4 `output.rs` tests pass; all 2 `iso_extract.rs` tests pass;
  all 2 `probe.rs::metadata_path_for` tests pass.
- `cargo test -p transcoderr --locked --lib --tests` passes (the
  pre-existing metrics flake notwithstanding).
- Manual end-to-end: an `.mp4` source transcoded through
  `hevc-normalize` lands at `<stem>.mkv` and the `.mp4` is deleted.
  An `.iso` source transcoded through the same flow still lands at
  `<stem>.mkv` and the `.iso` is deleted.

## Notify template behavior — minor user-visible shift

Today, after `iso.extract` mutates `ctx.file.path` to
`Movie.mkv`, the bundled flow's success-notify template
`"✓ {{ file.path }} normalized"` renders as
`"✓ Movie.mkv normalized"`. After this refactor, `ctx.file.path`
stays at the original `Movie.iso`, so the same template renders
`"✓ Movie.iso normalized"`.

For non-ISO inputs whose extension didn't change (e.g. `Movie.mkv →
Movie.mkv`), there's no behavior shift.

For non-ISO inputs whose extension DID change (e.g. `Movie.mp4 →
Movie.mkv`), today's renamed-but-broken file made the template
render `"✓ Movie.mp4 normalized"` (the .mp4 named file with MKV
bytes). After the refactor, the .mp4 is gone and the .mkv is in
place, but the template still says `"✓ Movie.mp4 normalized"` (the
source name).

This is acceptable. Both readings are sensible: "your Movie.iso /
Movie.mp4 was processed". A user who wants to render the destination
path can author a follow-up template variable; that's deferred.
