# HDR→SDR Tonemap Step — Design

**Date:** 2026-04-27
**Branch:** `feature/tonemap`
**Status:** Draft, pending implementation
**Builds on:** v0.8.3 (commit `661dd55`)

## Goal

Add a new built-in flow step `plan.video.tonemap` that detects HDR10 /
HLG sources from probe data and tells `plan.execute` to apply an
HDR→SDR tonemap to BT.709 with 8-bit (yuv420p) output. The step is a
no-op for SDR sources, so it's safe to add to any flow that re-encodes
video.

The user pain point: HDR content re-encoded by today's `hevc-normalize`
flow ends up with HDR metadata copied through, which renders
washed-out / desaturated on non-HDR displays. After this branch,
dropping `plan.video.tonemap` into the flow YAML produces SDR output
that plays correctly on any TV or phone.

## Non-goals

- libplacebo in the runtime image. Tracked in
  [#8](https://github.com/seanbarzilay/transcoderr/issues/8). The
  current `jrottenberg/ffmpeg:6.0-nvidia2204` build doesn't include it;
  `engine: auto` falls back to zscale today.
- Dolby Vision-aware tonemap (Profile 5; metadata-aware Profile 7/8).
  Profile 7/8 sources work via their HDR10 base layer; Profile 5
  passes through undetected.
- HLG-specific parameter tuning. PQ and HLG share the filter chain.
- Tonemap algorithm choice (hardcoded to `hable` for zscale, `auto`
  for libplacebo).
- Hardware-specific tonemap filters (`tonemap_cuda`, `tonemap_vaapi`,
  `tonemap_qsv`).
- Bundling the new step into `hevc-normalize.yaml`. Adding it is left
  as a deliberate follow-up.

## Design

### HDR detection helper

A pure helper in `crates/transcoderr/src/steps/plan_steps.rs` that
inspects the probe's first video stream:

```rust
/// Returns Some("hdr10") | Some("hlg") | None based on the first
/// video stream's color_transfer field. Dolby Vision detection (via
/// stream side data) is deferred — for now we treat DV the same as
/// the base HDR10 layer it falls back to.
fn detect_hdr_kind(probe: &serde_json::Value) -> Option<&'static str> {
    let streams = probe.get("streams")?.as_array()?;
    for s in streams {
        if s.get("codec_type")?.as_str()? != "video" { continue; }
        let transfer = s.get("color_transfer")?.as_str()?;
        return match transfer {
            "smpte2084" => Some("hdr10"),
            "arib-std-b67" => Some("hlg"),
            _ => None,
        };
    }
    None
}
```

Notes:

- `smpte2084` is the PQ transfer function — used by HDR10, HDR10+, and
  the HDR10 base layer of Dolby Vision Profile 7/8.
- `arib-std-b67` is HLG — used by broadcast HDR and some streaming.
- The `kind` string is logged in the run UI ("HDR10 source detected →
  tonemapping"). The tonemap chain itself doesn't branch on kind —
  PQ and HLG go through the same filter graph to BT.709.

### New `VideoPlan` field + `TonemapPlan`

`crates/transcoderr/src/flow/plan.rs` gains:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TonemapEngine {
    #[default]
    Auto,         // boot-probe picks libplacebo if present, else zscale
    Libplacebo,
    Zscale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TonemapPlan {
    pub engine: TonemapEngine,
    /// "hdr10" | "hlg" — logged for the run UI.
    pub source_kind: String,
}
```

`VideoPlan` adds `pub tonemap: Option<TonemapPlan>` (default `None`).
The struct already has `serde(default)` semantics via its `Default`
derive, so existing checkpoints continue to deserialize fine — missing
`tonemap` field deserializes to `None`.

### New step `plan.video.tonemap`

Lives in `crates/transcoderr/src/steps/plan_steps.rs`, registered in
`builtin.rs` between `plan.video.encode` and `plan.audio.ensure`:

```rust
pub struct PlanVideoTonemapStep;

#[async_trait]
impl Step for PlanVideoTonemapStep {
    fn name(&self) -> &'static str { "plan.video.tonemap" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let probe = ctx.probe.as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.video.tonemap: no probe data"))?
            .clone();

        let Some(kind) = detect_hdr_kind(&probe) else {
            on_progress(StepProgress::Log(
                "plan.video.tonemap: no HDR detected, skipping".into()
            ));
            return Ok(());
        };

        let engine: TonemapEngine = with.get("engine")
            .and_then(|v| v.as_str())
            .map(|s| serde_json::from_value(json!(s)).unwrap_or(TonemapEngine::Auto))
            .unwrap_or(TonemapEngine::Auto);

        let mut plan = require_plan(ctx)?;
        plan.video.tonemap = Some(TonemapPlan {
            engine,
            source_kind: kind.to_string(),
        });
        save_plan(ctx, &plan);

        on_progress(StepProgress::Log(format!(
            "plan.video.tonemap: {kind} source detected, engine={engine:?}"
        )));
        Ok(())
    }
}
```

Behavior:

- **No-op for SDR sources** — auto-skip, log once. The flow can include
  the step unconditionally; only HDR sources get tonemapped.
- **Idempotent** — running the step twice is the same as once.
- **Trusts flow ordering** — if the YAML places this BEFORE
  `plan.video.encode`, the encode step might overwrite parts of the
  plan; if it places it AFTER, the encode is configured first and
  tonemap layers on top. The recommended order is encode → tonemap.
  The step doesn't enforce ordering.
- **Single tunable** — `with: { engine: auto | libplacebo | zscale }`.
  Default `auto`.

### Boot-time ffmpeg-caps probe

New module `crates/transcoderr/src/ffmpeg_caps.rs`:

```rust
//! Boot-time probe of ffmpeg-binary capabilities. Currently surfaces
//! whether `libplacebo` is in the filter list — used by the tonemap
//! step to pick between libplacebo (preferred when present) and the
//! software zscale+tonemap chain (always available).

use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct FfmpegCaps {
    pub has_libplacebo: bool,
}

impl FfmpegCaps {
    pub async fn probe() -> Self {
        let out = match Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-filters")
            .output()
            .await
        {
            Ok(o) => o,
            Err(_) => return Self::default(),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        Self {
            has_libplacebo: stdout.lines().any(|l| {
                l.split_whitespace().nth(1) == Some("libplacebo")
            }),
        }
    }
}
```

`AppState` adds `pub ffmpeg_caps: Arc<FfmpegCaps>`. Populated once at
boot in `crates/transcoderr/src/main.rs::serve`, alongside the existing
`hw_caps` probe:

```rust
let ffmpeg_caps = std::sync::Arc::new(FfmpegCaps::probe().await);
tracing::info!(
    libplacebo = ffmpeg_caps.has_libplacebo,
    "ffmpeg caps probed",
);
```

`PlanExecuteStep` accesses `ffmpeg_caps` via the same plumbing it uses
for `hw: DeviceRegistry` today (held in the registered step instance).

For the current production runtime image, this logs
`libplacebo=false` and engine selection in `build_command` falls back
to zscale. The plumbing supports both engines transparently.

### `plan.execute` filter-chain construction

`crates/transcoderr/src/steps/plan_execute.rs::build_command` adds two
small extensions to the per-stream video output block:

```rust
fn build_tonemap_vf(engine: TonemapEngine, has_libplacebo: bool) -> &'static str {
    let resolved = match engine {
        TonemapEngine::Libplacebo => true,
        TonemapEngine::Zscale => false,
        TonemapEngine::Auto => has_libplacebo,
    };
    if resolved {
        "libplacebo=tonemapping=auto:colorspace=bt709:color_primaries=bt709:color_trc=bt709:format=yuv420p"
    } else {
        "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=tonemap=hable:desat=0,zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
    }
}
```

In `build_command`'s video-stream branch:

```rust
if let Some(tm) = &plan.video.tonemap {
    let vf = build_tonemap_vf(tm.engine, ffmpeg_caps.has_libplacebo);
    cmd.args([&format!("-filter:v:{v_out}"), vf]);
    cmd.args([&format!("-pix_fmt:v:{v_out}"), "yuv420p"]);
} else if force_10bit {
    // existing 10-bit preserve path (unchanged)
    cmd.args([&format!("-profile:v:{v_out}"), "main10"]);
    cmd.args([&format!("-pix_fmt:v:{v_out}"), "p010le"]);
}
```

Key invariants:

- Tonemap **overrides** the `preserve_10bit` flag. HDR→SDR fundamentally
  produces 8-bit output; preserving 10-bit conflicts with the BT.709
  target. The `else if` branch makes this explicit.
- The trailing `-pix_fmt yuv420p` is redundant with the filter chain's
  final `format=yuv420p` step but defends against any encoder
  defaulting to a higher bit depth on a 10-bit input.
- The filter is per-stream (`-filter:v:N`) to align with how
  `plan.execute` already emits per-stream codec args.

`build_command`'s signature gains a `&FfmpegCaps` parameter (or
equivalent). `PlanExecuteStep::execute` passes it from the step's
held reference.

### Flow YAML usage

Flow authors opt in by adding the step after `plan.video.encode`:

```yaml
- id: plan-encode-video
  use: plan.video.encode
  with:
    codec: x265
    crf: 19
    preset: fast

- id: plan-tonemap
  use: plan.video.tonemap
  # optionally: with: { engine: zscale }  # default is auto
```

For SDR inputs, `plan-tonemap` no-ops and the encode runs untouched.
For HDR inputs, the encode is augmented with the tonemap filter and
8-bit output.

This branch does NOT add the step to `hevc-normalize.yaml`. Operators
who want HDR→SDR conversion add it deliberately.

## Testing

**Pure-function tests** (cheap, hermetic):

- `detect_hdr_kind_returns_none_for_sdr_probe`
- `detect_hdr_kind_returns_hdr10_for_smpte2084`
- `detect_hdr_kind_returns_hlg_for_arib_std_b67`
- `detect_hdr_kind_ignores_audio_streams`
- `build_tonemap_vf_libplacebo_uses_libplacebo_filter`
- `build_tonemap_vf_zscale_uses_zscale_chain`
- `build_tonemap_vf_auto_picks_libplacebo_when_present`
- `build_tonemap_vf_auto_falls_back_to_zscale`

**Step-level tests** (uses `Context::for_file` + manual probe seed,
no subprocess):

- `tonemap_step_skips_sdr_source` — SDR probe → no `plan.video.tonemap`,
  one log emitted
- `tonemap_step_marks_hdr10_source` — `smpte2084` probe → `tonemap.is_some()`,
  `source_kind == "hdr10"`, `engine == Auto`
- `tonemap_step_respects_yaml_engine_override` — same HDR10 probe +
  `with: { engine: zscale }` → `engine == Zscale`

**Skipped:**

- `FfmpegCaps::probe` — depends on the real ffmpeg binary; not worth
  the stubbing complexity.
- `plan.execute` building the actual ffmpeg command including the
  filter — depends on the StreamPlan's full construction; the filter
  string is covered by `build_tonemap_vf` tests, and end-to-end is
  manual.

**Manual verification:**

- Drop an HDR10 source into a Radarr-watched dir; watch the run log
  `plan.video.tonemap: hdr10 source detected, engine=Auto`; confirm
  the encoded `.mkv` plays correctly on a non-HDR display.

## Acceptance

The branch is ready to merge when:

- `VideoPlan.tonemap` field exists; `TonemapEngine` and `TonemapPlan`
  types defined.
- `PlanVideoTonemapStep` registered as `plan.video.tonemap` in
  `builtin.rs`. SDR sources no-op; HDR sources mark the plan.
- `FfmpegCaps::probe` runs at boot in `main.rs::serve`; the resulting
  `Arc<FfmpegCaps>` is threaded through `registry::init` →
  `builtin::register_all` → `PlanExecuteStep::ffmpeg_caps` (mirroring
  the existing `DeviceRegistry` plumbing). Not stored in `AppState`
  because no HTTP handler currently needs it; if a future `/api/hw`
  expansion wants to surface filter caps, threading it through there
  is a one-line addition.
- `plan.execute::build_command` emits a `-filter:v:N` filter and
  `-pix_fmt yuv420p` when `plan.video.tonemap` is set; bypasses the
  10-bit preserve branch.
- All 8 pure-function tests + 3 step-level tests pass.
- `cargo test -p transcoderr --locked --lib --tests` passes (the
  pre-existing metrics flake notwithstanding).
- Manual verification: HDR10 input lands as SDR `.mkv`, plays on a
  non-HDR display without washed-out colors.
