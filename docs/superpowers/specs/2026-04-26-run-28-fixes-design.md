# Run-28 Failure Fixes — Design

**Date:** 2026-04-26
**Branch:** `feature/run-28-fixes`
**Status:** Draft, pending implementation

## Goal

Fix the cascade of bugs that caused production run #28 to fail with
`verify ffprobe failed: ... End of file`. Investigation traced the
failure to three independent issues that compounded:

1. **`SUPPORTED_SUB_CODECS` lists `mov_text` as MKV-compatible** when
   ffmpeg's MKV muxer actually rejects it.
2. **The bundled `hevc-normalize` flow doesn't invoke
   `plan.subs.drop_unsupported`**, so even with a correct allow-list
   the flow doesn't act on the result.
3. **`plan_execute.rs` discards the CPU fallback's exit status**, so
   when both NVENC and the CPU retry hit the same muxer-init error,
   the step reports success and a 313-byte broken `.mkv` advances to
   `verify.playable`.

After this branch, an MP4 with a `mov_text` subtitle track transcodes
through `hevc-normalize` cleanly: the mov_text stream is dropped before
plan.execute runs, ffmpeg gets `-c:s copy` only on streams MKV can
accept, and the encode succeeds.

## Non-goals

- Transcoding `mov_text` to `srt` to preserve the subtitles. The
  current `plan.subs.drop_unsupported` step is drop-only; adding a
  transcode mode requires extending the StreamPlan model and is
  deferred until users complain.
- Per-container subtitle compatibility tables. Today `SUPPORTED_SUB_CODECS`
  is a flat list assuming MKV output. When other output containers
  are added, this needs to become a per-container map.
- Smarter NVENC fault detection (parsing ffmpeg stderr for known
  NVENC-specific signatures to skip wasted CPU retries on non-HW
  errors). Fragile, low payoff.
- Auditing every other codec in `SUPPORTED_SUB_CODECS` for actual
  MKV-mux compatibility. Anything turning out to be wrong is a
  separate one-line follow-up.
- Output validation inside `plan.execute` (file size > N bytes,
  roundtrip ffprobe). `verify.playable` is supposed to catch broken
  outputs; bug #3's fix closes the hole that let bugs #1+#2 reach it.

## Design

### Bug 1: Remove `mov_text` from `SUPPORTED_SUB_CODECS`

In `crates/transcoderr/src/steps/plan_steps.rs`, the constant at
lines 14-26 currently includes `"mov_text"`:

```rust
const SUPPORTED_SUB_CODECS: &[&str] = &[
    "srt",
    "subrip",
    "ass",
    "ssa",
    "mov_text",
    "hdmv_pgs_subtitle",
    ...
];
```

After:

```rust
/// Subtitle codecs that ffmpeg can mux into Matroska (`-c:s copy` to mkv).
/// Notably absent: `mov_text` — it's the MP4-native text-subs format and
/// the MKV muxer rejects it with "Function not implemented" at header
/// write time. plan.subs.drop_unsupported drops mov_text streams.
const SUPPORTED_SUB_CODECS: &[&str] = &[
    "srt",
    "subrip",
    "ass",
    "ssa",
    "hdmv_pgs_subtitle",
    "pgssub",
    "dvd_subtitle",
    "dvdsub",
    "dvb_subtitle",
    "webvtt",
];
```

The doc-comment is the load-bearing change in spirit: it documents WHY
mov_text isn't on the list so a future contributor doesn't add it back
thinking it was an omission.

### Bug 2: Add `plan.subs.drop_unsupported` to `hevc-normalize.yaml`

In `docs/flows/hevc-normalize.yaml`, insert a new step between
`plan-drop-data` and `codec-gate`:

```yaml
  - id: plan-drop-data
    use: plan.streams.drop_data

  # Drop subtitle streams whose codec doesn't mux into the planned container
  # (e.g. mov_text from MP4 sources can't go into MKV). Without this, ffmpeg
  # bails at MKV muxer init with "Function not implemented" before any
  # encoding starts, and the broken near-empty tmp file confuses verify.
  - id: plan-drop-unsupported-subs
    use: plan.subs.drop_unsupported

  - id: codec-gate
    if: probe.streams[0].codec_name == "hevc"
    ...
```

Placement rationale: *after* `drop_data` and `drop_cover_art` (the
"cull obviously-unwanted streams" prefix); *before* `codec-gate` so the
check runs whether or not we re-encode video — a copy-only pass with
mov_text → MKV would fail just the same.

### Bug 3: Check the CPU fallback's exit status in `plan_execute.rs`

`crates/transcoderr/src/steps/plan_execute.rs` currently has (lines
~106-136):

```rust
Ok(status) => {
    if acquired_key.is_some() && plan.video.hw_fallback_cpu {
        on_progress(StepProgress::Marker {
            kind: "hw_runtime_failure".into(),
            payload: json!({ "device": acquired_key }),
        });
        let _ = std::fs::remove_file(&dest);
        let cpu_cmd = build_command(&src, &dest, &plan, &probe, None)?;
        let _ = crate::ffmpeg::run_with_live_events(   // BUG: discards status
            cpu_cmd,
            duration_sec,
            ctx.cancel.as_ref(),
            |ev| match ev {
                FfmpegEvent::Pct(p) => on_progress(StepProgress::Pct(p)),
                FfmpegEvent::Line(l) => {
                    on_progress(StepProgress::Log(format!("ffmpeg: {l}")))
                }
            },
        )
        .await?;
        staging::record_output(
            ctx,
            &dest,
            json!({ "hw": null, "fallback_from": acquired_key }),
        );
        Ok(())
    } else {
        anyhow::bail!("plan.execute: ffmpeg exited {:?}", status.code())
    }
}
```

After: bind the result, mirror the success-path's `.success()` check,
and bail with both exit codes for diagnostic clarity:

```rust
Ok(status) => {
    if acquired_key.is_some() && plan.video.hw_fallback_cpu {
        on_progress(StepProgress::Marker {
            kind: "hw_runtime_failure".into(),
            payload: json!({ "device": acquired_key }),
        });
        let _ = std::fs::remove_file(&dest);
        let cpu_cmd = build_command(&src, &dest, &plan, &probe, None)?;
        let cpu_status = crate::ffmpeg::run_with_live_events(
            cpu_cmd,
            duration_sec,
            ctx.cancel.as_ref(),
            |ev| match ev {
                FfmpegEvent::Pct(p) => on_progress(StepProgress::Pct(p)),
                FfmpegEvent::Line(l) => {
                    on_progress(StepProgress::Log(format!("ffmpeg: {l}")))
                }
            },
        )
        .await?;
        if !cpu_status.success() {
            anyhow::bail!(
                "plan.execute: cpu fallback also failed (hw exited {:?}, cpu exited {:?})",
                status.code(),
                cpu_status.code()
            );
        }
        staging::record_output(
            ctx,
            &dest,
            json!({ "hw": null, "fallback_from": acquired_key }),
        );
        Ok(())
    } else {
        anyhow::bail!("plan.execute: ffmpeg exited {:?}", status.code())
    }
}
```

When an operator sees `hw exited 234, cpu exited 234` they immediately
know it's not an NVENC fault — same exit code on both attempts means
the failure is upstream of the encoder.

The behavior when only NVENC fails (transient GPU fault) is unchanged:
CPU fallback runs, succeeds, the staged tmp is recorded, the step
returns Ok.

## Testing

**Bug 1**: no automated test. The change is data, not logic. The
existing `plan.subs.drop_unsupported` step covers the consumer side;
once the data is correct, the step does the right thing. An "assert
mov_text is not in the list" test would just restate the data.

**Bug 2**: no new unit test. The flow-parsing test that loads bundled
YAMLs at boot would fail if the new step were misspelled or referenced
an unregistered name. Real proof is manual: drop an MP4 with mov_text
into the radarr-watched dir and watch it transcode cleanly.

**Bug 3**: skipped — testing the fallback-fails-too case requires
spawning real ffmpeg with both a working HW accelerator that fails AND
a CPU fallback that also fails. Refactoring `run_with_live_events`
behind a trait to stub both is significant surface for low payoff. The
`cpu_status.success()` check is straightforward enough that code
review catches drift, and the failure mode it guards against will
surface loudly the next time it triggers.

End-to-end verification is manual: re-trigger run-28's MP4 (or any
MP4 with `mov_text`) on the dev server, confirm it now reaches
`output:replace`.

## Acceptance

The branch is ready to merge when:

- `crates/transcoderr/src/steps/plan_steps.rs::SUPPORTED_SUB_CODECS`
  no longer contains `"mov_text"` and has the documenting comment.
- `docs/flows/hevc-normalize.yaml` invokes
  `plan.subs.drop_unsupported` between `plan-drop-data` and `codec-gate`.
- `plan_execute.rs` captures the CPU fallback's exit status and bails
  on `!success()` with a message that includes both exit codes.
- `cargo test -p transcoderr --locked --lib --tests` passes (the
  pre-existing metrics flake notwithstanding).
