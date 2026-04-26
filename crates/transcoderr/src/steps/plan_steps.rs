//! All `plan.*` mutator steps. Each one tweaks `ctx.steps["_plan"]` (the
//! `StreamPlan`) without spawning ffmpeg. The single `plan.execute` step
//! (in `plan_execute.rs`) materializes the plan into one ffmpeg invocation.

use crate::flow::plan::{require_plan, save_plan, AddedAudio, StreamPlan, VideoMode};
use crate::flow::Context;
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;

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

const PLAYABLE_AUDIO: &[&str] = &["aac", "ac3", "eac3", "mp3", "opus"];

/// Commentary tracks shouldn't satisfy the "wanted audio codec" check — a
/// director's commentary in AC3 6ch eng is not a usable main audio track,
/// even though it matches the codec/channels/language spec on paper. We
/// detect commentary by either the `disposition.comment=1` flag (set by some
/// muxers) or a "comment(ary)" substring in the title.
fn is_commentary(s: &Value) -> bool {
    let comment_disp = s
        .get("disposition")
        .and_then(|d| d.get("comment"))
        .and_then(|v| v.as_i64())
        == Some(1);
    let title = s
        .get("tags")
        .and_then(|t| t.get("title"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    comment_disp || title.contains("comment")
}

fn channel_layout_label(channels: i64) -> String {
    match channels {
        1 => "Mono".into(),
        2 => "Stereo".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n}ch"),
    }
}

/// Returns `Some("hdr10")` | `Some("hlg")` | `None` based on the first
/// video stream's `color_transfer` field. Dolby Vision detection (via
/// stream side data) is deferred — for now we treat DV the same as the
/// base HDR10 layer it falls back to.
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

// ---------------------------------------------------------------------------
// plan.init — seeds StreamPlan from probe (every stream copied, container=mkv)
// ---------------------------------------------------------------------------

pub struct PlanInitStep;

#[async_trait]
impl Step for PlanInitStep {
    fn name(&self) -> &'static str { "plan.init" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.init: no probe data; run `probe` first"))?;
        let plan = StreamPlan::from_probe(probe);
        let kept = plan.kept_indices().len();
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.init: seeded plan with {} streams, container=mkv, video=copy",
            kept
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.input.tolerate_errors — append -err_detect ignore_err -fflags +discardcorrupt
// ---------------------------------------------------------------------------

pub struct PlanTolerateErrorsStep;

#[async_trait]
impl Step for PlanTolerateErrorsStep {
    fn name(&self) -> &'static str { "plan.input.tolerate_errors" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let mut plan = require_plan(ctx)?;
        for arg in ["-err_detect", "ignore_err", "-fflags", "+discardcorrupt"] {
            if !plan.global_input_args.iter().any(|a| a == arg) {
                plan.global_input_args.push(arg.to_string());
            }
        }
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(
            "plan.input.tolerate_errors: enabled".into(),
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.streams.drop_cover_art — mark video streams with attached_pic=1 as removed
// ---------------------------------------------------------------------------

pub struct PlanDropCoverArtStep;

#[async_trait]
impl Step for PlanDropCoverArtStep {
    fn name(&self) -> &'static str { "plan.streams.drop_cover_art" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.streams.drop_cover_art: no probe data"))?
            .clone();
        let mut plan = require_plan(ctx)?;
        let dropped = plan.drop_streams_where(&probe, |s| {
            s.get("disposition")
                .and_then(|d| d.get("attached_pic"))
                .and_then(|v| v.as_i64())
                == Some(1)
        });
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.streams.drop_cover_art: dropped {dropped} cover-art stream(s)"
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.streams.drop_data — drop every data stream
// ---------------------------------------------------------------------------

pub struct PlanDropDataStep;

#[async_trait]
impl Step for PlanDropDataStep {
    fn name(&self) -> &'static str { "plan.streams.drop_data" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.streams.drop_data: no probe data"))?
            .clone();
        let mut plan = require_plan(ctx)?;
        let dropped = plan.drop_streams_where(&probe, |s| {
            s.get("codec_type").and_then(|v| v.as_str()) == Some("data")
        });
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.streams.drop_data: dropped {dropped} data stream(s)"
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.subs.drop_unsupported — drop subtitles whose codec isn't in the allowlist
// ---------------------------------------------------------------------------

pub struct PlanDropUnsupportedSubsStep;

#[async_trait]
impl Step for PlanDropUnsupportedSubsStep {
    fn name(&self) -> &'static str { "plan.subs.drop_unsupported" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.subs.drop_unsupported: no probe data"))?
            .clone();
        let mut plan = require_plan(ctx)?;
        let dropped = plan.drop_streams_where(&probe, |s| {
            if s.get("codec_type").and_then(|v| v.as_str()) != Some("subtitle") {
                return false;
            }
            let codec = s
                .get("codec_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            !SUPPORTED_SUB_CODECS.contains(&codec.as_str())
        });
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.subs.drop_unsupported: dropped {dropped} unsupported subtitle stream(s)"
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.container — set output container (e.g. "mkv", "mp4")
// ---------------------------------------------------------------------------

pub struct PlanContainerStep;

#[async_trait]
impl Step for PlanContainerStep {
    fn name(&self) -> &'static str { "plan.container" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let container = with
            .get("container")
            .or_else(|| with.get("name"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("plan.container: missing `container` (or `name`) — e.g. mkv, mp4")
            })?;
        let mut plan = require_plan(ctx)?;
        plan.container = container.to_string();
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.container: {container}"
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.video.encode — mark video for re-encode (codec/crf/preset/hw/preserve_10bit)
// ---------------------------------------------------------------------------

pub struct PlanVideoEncodeStep;

#[async_trait]
impl Step for PlanVideoEncodeStep {
    fn name(&self) -> &'static str { "plan.video.encode" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let codec = with
            .get("codec")
            .and_then(|v| v.as_str())
            .unwrap_or("x265")
            .to_string();
        let crf = with.get("crf").and_then(|v| v.as_i64());
        let preset = with
            .get("preset")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let preserve_10bit = with
            .get("preserve_10bit")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let hw_block = with.get("hw").cloned().unwrap_or(Value::Null);
        let prefer = hw_block
            .get("prefer")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let cpu_fallback =
            hw_block.get("fallback").and_then(|v| v.as_str()) == Some("cpu");

        let mut plan = require_plan(ctx)?;
        plan.video.mode = VideoMode::Encode { codec: codec.clone() };
        plan.video.crf = crf;
        plan.video.preset = preset;
        plan.video.preserve_10bit = preserve_10bit;
        plan.video.hw_prefer = prefer.clone();
        plan.video.hw_fallback_cpu = cpu_fallback;
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.video.encode: {codec} crf={:?} preset={:?} hw_prefer={:?} fallback_cpu={cpu_fallback}",
            crf, plan.video.preset, prefer
        )));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// plan.audio.ensure — add a transcoded audio stream if no existing track meets the spec
// ---------------------------------------------------------------------------

pub struct PlanAudioEnsureStep;

#[async_trait]
impl Step for PlanAudioEnsureStep {
    fn name(&self) -> &'static str { "plan.audio.ensure" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let target_codec = with
            .get("codec")
            .and_then(|v| v.as_str())
            .unwrap_or("ac3")
            .to_string();
        let target_lang = with
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("eng")
            .to_string();
        let target_channels = with
            .get("channels")
            .and_then(|v| v.as_i64())
            .unwrap_or(6);
        let dedupe = with
            .get("dedupe")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.audio.ensure: no probe data"))?
            .clone();
        let mut plan = require_plan(ctx)?;

        let streams = probe
            .get("streams")
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();

        // Existing audio tracks (kept ones only — we don't want to dedupe against
        // a track another step has already marked for removal).
        let existing_audio = streams.iter().filter(|s| {
            let idx = s.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
            let kept = plan.stream_keep.get(&idx).copied().unwrap_or(true);
            kept && s.get("codec_type").and_then(|v| v.as_str()) == Some("audio")
        });

        let has_target = existing_audio.clone().any(|s| {
            // Commentary tracks don't count as the wanted main audio even if
            // they happen to match codec/channels/language.
            if is_commentary(s) {
                return false;
            }
            let codec = s
                .get("codec_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            let ch = s.get("channels").and_then(|v| v.as_i64()).unwrap_or(0);
            let lang = s
                .get("tags")
                .and_then(|t| t.get("language"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase();
            codec == target_codec
                && ch >= target_channels
                && (lang == target_lang || lang.is_empty() || lang == "und")
        });

        if has_target {
            on_progress(StepProgress::Log(format!(
                "plan.audio.ensure: existing {target_codec} {target_channels}ch [{target_lang}] track found; nothing to add"
            )));
            save_plan(ctx, &plan);
            return Ok(());
        }

        // Pick highest-channel non-commentary audio stream as seed.
        let seed = existing_audio
            .clone()
            .filter(|s| !is_commentary(s))
            .max_by_key(|s| s.get("channels").and_then(|v| v.as_i64()).unwrap_or(0))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "plan.audio.ensure: no non-commentary audio stream to seed from"
                )
            })?;
        let seed_index = seed.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
        let seed_ch = seed.get("channels").and_then(|v| v.as_i64()).unwrap_or(0);

        // Dedupe: skip add when an existing playable track already covers the
        // target channel count.
        if dedupe {
            let playable_max = existing_audio
                .filter(|s| !is_commentary(s))
                .filter(|s| {
                    let codec = s
                        .get("codec_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    PLAYABLE_AUDIO.contains(&codec.as_str())
                })
                .filter_map(|s| s.get("channels").and_then(|v| v.as_i64()))
                .max()
                .unwrap_or(0);
            if target_channels <= playable_max {
                on_progress(StepProgress::Log(format!(
                    "plan.audio.ensure: dedupe skip (existing playable {playable_max}ch >= target {target_channels}ch)"
                )));
                save_plan(ctx, &plan);
                return Ok(());
            }
        }

        let title = format!(
            "{} {}",
            target_codec.to_uppercase(),
            channel_layout_label(target_channels)
        );
        plan.audio_added.push(AddedAudio {
            seed_index,
            codec: target_codec.clone(),
            channels: target_channels,
            language: target_lang.clone(),
            title: title.clone(),
        });
        save_plan(ctx, &plan);
        on_progress(StepProgress::Log(format!(
            "plan.audio.ensure: + {target_codec} {target_channels}ch [{target_lang}] from source stream {seed_index} (seed had {seed_ch}ch), title={title:?}"
        )));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::plan::{load_plan, save_plan, StreamPlan};
    use serde_json::json;

    #[tokio::test]
    async fn plan_init_seeds_from_probe() {
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264"},
                {"index": 1, "codec_type": "audio", "codec_name": "aac"},
                {"index": 2, "codec_type": "subtitle", "codec_name": "subrip"},
            ]
        }));
        let mut events = vec![];
        let mut cb = |e: StepProgress| events.push(e);
        PlanInitStep.execute(&Default::default(), &mut ctx, &mut cb).await.unwrap();
        let plan = load_plan(&ctx).unwrap();
        assert_eq!(plan.kept_indices(), vec![0, 1, 2]);
        assert_eq!(plan.container, "mkv");
        assert!(matches!(plan.video.mode, VideoMode::Copy));
    }

    #[tokio::test]
    async fn plan_drop_cover_art_marks_attached_pic() {
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264", "disposition": {}},
                {"index": 1, "codec_type": "video", "codec_name": "mjpeg", "disposition": {"attached_pic": 1}},
                {"index": 2, "codec_type": "audio", "codec_name": "aac"},
            ]
        }));
        let plan = StreamPlan::from_probe(ctx.probe.as_ref().unwrap()); save_plan(&mut ctx, &plan);
        let mut cb = |_: StepProgress| {};
        PlanDropCoverArtStep.execute(&Default::default(), &mut ctx, &mut cb).await.unwrap();
        let plan = load_plan(&ctx).unwrap();
        assert_eq!(plan.kept_indices(), vec![0, 2]);
    }

    #[tokio::test]
    async fn plan_audio_ensure_adds_when_missing() {
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video"},
                {"index": 1, "codec_type": "audio", "codec_name": "dts", "channels": 6, "tags": {"language": "eng"}},
            ]
        }));
        let plan = StreamPlan::from_probe(ctx.probe.as_ref().unwrap()); save_plan(&mut ctx, &plan);
        let mut with: BTreeMap<String, Value> = BTreeMap::new();
        with.insert("codec".into(), json!("ac3"));
        with.insert("channels".into(), json!(6));
        with.insert("language".into(), json!("eng"));
        with.insert("dedupe".into(), json!(false));
        let mut cb = |_: StepProgress| {};
        PlanAudioEnsureStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
        let plan = load_plan(&ctx).unwrap();
        assert_eq!(plan.audio_added.len(), 1);
        assert_eq!(plan.audio_added[0].codec, "ac3");
        assert_eq!(plan.audio_added[0].title, "AC3 5.1");
    }

    #[tokio::test]
    async fn plan_audio_ensure_skips_when_already_present() {
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video"},
                {"index": 1, "codec_type": "audio", "codec_name": "ac3", "channels": 6, "tags": {"language": "eng"}},
            ]
        }));
        let plan = StreamPlan::from_probe(ctx.probe.as_ref().unwrap()); save_plan(&mut ctx, &plan);
        let mut with: BTreeMap<String, Value> = BTreeMap::new();
        with.insert("codec".into(), json!("ac3"));
        with.insert("channels".into(), json!(6));
        with.insert("language".into(), json!("eng"));
        let mut cb = |_: StepProgress| {};
        PlanAudioEnsureStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
        let plan = load_plan(&ctx).unwrap();
        assert!(plan.audio_added.is_empty());
    }

    #[tokio::test]
    async fn plan_audio_ensure_does_not_count_commentary_as_target() {
        // Source has DTS-HD MA 5.1 main audio + an AC3 6ch eng COMMENTARY track.
        // The target is AC3 6ch eng. The commentary matches on paper but it's
        // not a usable main track, so audio.ensure must still add a real one.
        // Both commentary and the new track should be present in the plan.
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video", "codec_name": "h264"},
                {"index": 1, "codec_type": "audio", "codec_name": "dts", "channels": 6,
                 "tags": {"language": "eng", "title": "DTS-HD MA 5.1"}},
                {"index": 2, "codec_type": "audio", "codec_name": "ac3", "channels": 6,
                 "tags": {"language": "eng", "title": "Director's Commentary"},
                 "disposition": {"comment": 1}},
            ]
        }));
        let plan = StreamPlan::from_probe(ctx.probe.as_ref().unwrap());
        save_plan(&mut ctx, &plan);
        let mut with: BTreeMap<String, Value> = BTreeMap::new();
        with.insert("codec".into(), json!("ac3"));
        with.insert("channels".into(), json!(6));
        with.insert("language".into(), json!("eng"));
        with.insert("dedupe".into(), json!(false));
        let mut cb = |_: StepProgress| {};
        PlanAudioEnsureStep.execute(&with, &mut ctx, &mut cb).await.unwrap();

        let plan = load_plan(&ctx).unwrap();
        // The commentary did NOT satisfy has_target → a new AC3 6ch was added.
        assert_eq!(plan.audio_added.len(), 1);
        // Seed must be the main DTS track (idx 1), not the commentary (idx 2).
        assert_eq!(plan.audio_added[0].seed_index, 1);
        // Both original audio streams (main + commentary) are still kept.
        assert_eq!(plan.kept_indices(), vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn plan_audio_ensure_commentary_detected_via_title_substring() {
        // Some encodes don't set disposition.comment but put "commentary" in
        // the title. The title-substring fallback should still catch it.
        let mut ctx = crate::flow::Context::for_file("/x");
        ctx.probe = Some(json!({
            "streams": [
                {"index": 0, "codec_type": "video"},
                {"index": 1, "codec_type": "audio", "codec_name": "dts", "channels": 6,
                 "tags": {"language": "eng"}},
                {"index": 2, "codec_type": "audio", "codec_name": "ac3", "channels": 6,
                 "tags": {"language": "eng", "title": "Filmmakers' Commentary"}},
            ]
        }));
        let plan = StreamPlan::from_probe(ctx.probe.as_ref().unwrap());
        save_plan(&mut ctx, &plan);
        let mut with: BTreeMap<String, Value> = BTreeMap::new();
        with.insert("codec".into(), json!("ac3"));
        with.insert("channels".into(), json!(6));
        with.insert("language".into(), json!("eng"));
        with.insert("dedupe".into(), json!(false));
        let mut cb = |_: StepProgress| {};
        PlanAudioEnsureStep.execute(&with, &mut ctx, &mut cb).await.unwrap();
        let plan = load_plan(&ctx).unwrap();
        assert_eq!(plan.audio_added.len(), 1, "commentary by title should not satisfy target");
    }

    #[test]
    fn detect_hdr_kind_returns_none_for_sdr_probe() {
        let probe = json!({
            "streams": [{
                "codec_type": "video",
                "color_transfer": "bt709"
            }]
        });
        assert!(detect_hdr_kind(&probe).is_none());
    }

    #[test]
    fn detect_hdr_kind_returns_hdr10_for_smpte2084() {
        let probe = json!({
            "streams": [{
                "codec_type": "video",
                "color_transfer": "smpte2084"
            }]
        });
        assert_eq!(detect_hdr_kind(&probe), Some("hdr10"));
    }

    #[test]
    fn detect_hdr_kind_returns_hlg_for_arib_std_b67() {
        let probe = json!({
            "streams": [{
                "codec_type": "video",
                "color_transfer": "arib-std-b67"
            }]
        });
        assert_eq!(detect_hdr_kind(&probe), Some("hlg"));
    }

    #[test]
    fn detect_hdr_kind_ignores_audio_streams() {
        let probe = json!({
            "streams": [{
                "codec_type": "audio",
                "channels": 6
            }]
        });
        assert!(detect_hdr_kind(&probe).is_none());
    }
}
