//! Typed `with:` config structs for built-in steps, used purely for
//! JSON-schema generation surfaced through `GET /api/step-kinds` and
//! the `list_step_kinds` MCP tool.
//!
//! Why a parallel module instead of refactoring each step to use these
//! structs directly: the steps were written against `BTreeMap<String,
//! Value>` and parse keys ad-hoc. Refactoring all of them in one PR
//! would be a high-risk change that's easy to subtly break (default
//! values, type coercion, optional vs required). The parallel approach
//! lets us emit accurate schemas immediately, with the trade-off that
//! a future change to a step's parsing must update its struct here too
//! to avoid drift. The integration tests against well-known steps will
//! catch egregious drift.

use schemars::{schema_for, JsonSchema};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// transcode — single-pass ffmpeg transcode (legacy chained step). Most flows
// today use the plan.* pipeline instead.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct TranscodeConfig {
    /// Software codec name. Default `x265`.
    #[serde(default)]
    pub codec: Option<String>,
    /// Constant rate factor. Default `22`.
    #[serde(default)]
    pub crf: Option<i64>,
    /// Hardware-accel preference block.
    #[serde(default)]
    pub hw: Option<HwBlock>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct HwBlock {
    /// Ordered list of preferred accels. Each is a string from
    /// `nvenc | qsv | vaapi | videotoolbox`.
    #[serde(default)]
    pub prefer: Option<Vec<String>>,
    /// Fallback when no preferred accel is available. Currently only
    /// `cpu` is meaningful — anything else disables fallback.
    #[serde(default)]
    pub fallback: Option<String>,
}

pub fn transcode_schema() -> Value {
    schema::<TranscodeConfig>()
}

// ---------------------------------------------------------------------------
// output — final-rename step.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct OutputConfig {
    /// `replace` (default) renames the staged output over the source
    /// path. `alongside` writes the new file beside the source and
    /// keeps the source intact.
    #[serde(default)]
    pub mode: Option<OutputMode>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum OutputMode {
    Replace,
    Alongside,
}

pub fn output_schema() -> Value {
    schema::<OutputConfig>()
}

// ---------------------------------------------------------------------------
// verify.playable — final correctness gate after transcode.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct VerifyPlayableConfig {
    /// Minimum acceptable duration ratio of the staged output to the
    /// source. Default `0.99` — anything shorter fails the verify.
    #[serde(default)]
    pub min_duration_ratio: Option<f64>,
}

pub fn verify_playable_schema() -> Value {
    schema::<VerifyPlayableConfig>()
}

// ---------------------------------------------------------------------------
// notify — fan a templated message out to a notifier by `channel:` name.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct NotifyConfig {
    /// Notifier name (matches a row in `notifiers.name`).
    pub channel: String,
    /// Optional message template. Supports `{{ ... }}` placeholders.
    /// When omitted, the notifier emits its kind-specific default
    /// (e.g. Jellyfin posts a library scan).
    #[serde(default)]
    pub template: Option<String>,
    /// Optional path to a file to attach. Currently only honored by a
    /// subset of notifier kinds.
    #[serde(default)]
    pub file: Option<String>,
}

pub fn notify_schema() -> Value {
    schema::<NotifyConfig>()
}

// ---------------------------------------------------------------------------
// webhook — fire an HTTP request with templated url/headers/body.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct WebhookConfig {
    /// Request URL. May contain `{{ ... }}` placeholders.
    pub url: String,
    /// HTTP method. One of `GET POST PUT PATCH DELETE`. Default `POST`.
    #[serde(default)]
    pub method: Option<String>,
    /// Header map. Values may contain `{{ ... }}` placeholders.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Optional request body. May contain `{{ ... }}` placeholders.
    #[serde(default)]
    pub body: Option<String>,
    /// Per-request timeout in seconds. Default `30`.
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// When true, network errors and non-2xx responses log a warning
    /// instead of failing the step.
    #[serde(default)]
    pub ignore_errors: Option<bool>,
}

pub fn webhook_schema() -> Value {
    schema::<WebhookConfig>()
}

// ---------------------------------------------------------------------------
// audio.ensure — legacy single-pass ensure-audio (the plan.* equivalent is
// `plan.audio.ensure`).
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct AudioEnsureConfig {
    /// Target audio codec, e.g. `ac3`, `eac3`, `aac`.
    #[serde(default)]
    pub codec: Option<String>,
    /// Target channel count. Default `6`.
    #[serde(default)]
    pub channels: Option<i64>,
    /// Target language tag. Default `eng`.
    #[serde(default)]
    pub language: Option<String>,
}

pub fn audio_ensure_schema() -> Value {
    schema::<AudioEnsureConfig>()
}

// ---------------------------------------------------------------------------
// remux — change container without re-encoding any streams.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct RemuxConfig {
    /// Destination container, e.g. `mkv`, `mp4`. Default `mkv`.
    #[serde(default)]
    pub container: Option<String>,
}

pub fn remux_schema() -> Value {
    schema::<RemuxConfig>()
}

// ---------------------------------------------------------------------------
// extract.subs — pull soft subs out of the source as a sidecar.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct ExtractSubsConfig {
    /// Subtitle stream language to extract. Default `eng`.
    #[serde(default)]
    pub language: Option<String>,
    /// Output codec for the sidecar. Default `srt`.
    #[serde(default)]
    pub codec: Option<String>,
}

pub fn extract_subs_schema() -> Value {
    schema::<ExtractSubsConfig>()
}

// ---------------------------------------------------------------------------
// strip.tracks — drop unwanted audio / subtitle streams.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct StripTracksConfig {
    /// Audio language tags to keep. Streams in any other language are
    /// dropped.
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    /// When true, also drops attached_pic / cover-art video streams.
    #[serde(default)]
    pub remove_cover_art: Option<bool>,
    /// When true, drops subtitle streams whose codec doesn't mux into
    /// the target container.
    #[serde(default)]
    pub drop_unsupported_subs: Option<bool>,
}

pub fn strip_tracks_schema() -> Value {
    schema::<StripTracksConfig>()
}

// ---------------------------------------------------------------------------
// move / copy — filesystem operations with one required `to` field.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct DestinationConfig {
    /// Destination path. May contain `{{ ... }}` placeholders.
    pub to: String,
}

pub fn destination_schema() -> Value {
    schema::<DestinationConfig>()
}

// ---------------------------------------------------------------------------
// shell — execute a shell command. `cmd` is the only key.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct ShellConfig {
    /// Command line. Runs via `sh -c`. May contain `{{ ... }}`
    /// placeholders.
    pub cmd: String,
}

pub fn shell_schema() -> Value {
    schema::<ShellConfig>()
}

// ---------------------------------------------------------------------------
// plan.* — declarative pipeline. Most plan-* steps take no `with:`; only
// the encode/audio/container/tonemap steps configure anything.
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PlanContainerConfig {
    /// Target container, e.g. `mkv`, `mp4`. Default `mkv`.
    #[serde(default)]
    pub format: Option<String>,
}

pub fn plan_container_schema() -> Value {
    schema::<PlanContainerConfig>()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PlanVideoEncodeConfig {
    /// Software codec name. Default `x265`.
    #[serde(default)]
    pub codec: Option<String>,
    /// CRF (quality). Lower is higher quality. Default `22`.
    #[serde(default)]
    pub crf: Option<i64>,
    /// Encoder preset (e.g. `fast`, `medium`, `slow`). Maps to the
    /// corresponding ffmpeg preset for the chosen encoder.
    #[serde(default)]
    pub preset: Option<String>,
    /// Preserve 10-bit color depth when re-encoding. Default `false`.
    #[serde(default)]
    pub preserve_10bit: Option<bool>,
    /// Hardware-accel preference block.
    #[serde(default)]
    pub hw: Option<HwBlock>,
}

pub fn plan_video_encode_schema() -> Value {
    schema::<PlanVideoEncodeConfig>()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PlanAudioEnsureConfig {
    /// Target audio codec, e.g. `ac3`, `eac3`, `aac`.
    #[serde(default)]
    pub codec: Option<String>,
    /// Target channel count. Default `6`.
    #[serde(default)]
    pub channels: Option<i64>,
    /// Target language tag. Default `eng`.
    #[serde(default)]
    pub language: Option<String>,
    /// When true, skip adding a track if an existing playable stream
    /// already covers the target channel count. Default `true`.
    #[serde(default)]
    pub dedupe: Option<bool>,
}

pub fn plan_audio_ensure_schema() -> Value {
    schema::<PlanAudioEnsureConfig>()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct PlanVideoTonemapConfig {
    /// Tone-mapping mode. Common values: `auto`, `hdr10`, `hlg`. The
    /// underlying ffmpeg filter is selected from the source's color
    /// transfer.
    #[serde(default)]
    pub mode: Option<String>,
}

pub fn plan_video_tonemap_schema() -> Value {
    schema::<PlanVideoTonemapConfig>()
}

// Steps that take no `with:` keys at all. Returning a strict empty
// object makes "this step accepts no config" explicit instead of
// silently ignoring stray keys at the schema layer.
pub fn empty_schema() -> Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "additionalProperties": false
    })
}
