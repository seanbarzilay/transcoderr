//! `plan.execute` — materializes the accumulated `StreamPlan` into ONE ffmpeg
//! invocation. This is the only step that actually re-reads / re-writes the
//! file in the new "plan-then-execute" pipeline.

use crate::ffmpeg::FfmpegEvent;
use crate::flow::plan::{require_plan, VideoMode, TonemapEngine};
use crate::flow::{staging, Context};
use crate::hw::{devices::Accel, semaphores::DeviceRegistry};
use crate::steps::{Step, StepProgress};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::process::Command;

/// Build the `-filter:v` value for an HDR→SDR tonemap. Picks libplacebo
/// when the engine is `Libplacebo`, or `Auto` and the boot probe found
/// libplacebo in the local ffmpeg. Otherwise returns the zscale chain
/// (always available — uses ffmpeg's built-in `tonemap` + `zscale`
/// filters).
pub(crate) fn build_tonemap_vf(engine: TonemapEngine, has_libplacebo: bool) -> &'static str {
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

pub struct PlanExecuteStep {
    pub hw: DeviceRegistry,
    pub ffmpeg_caps: std::sync::Arc<crate::ffmpeg_caps::FfmpegCaps>,
}

#[async_trait]
impl Step for PlanExecuteStep {
    fn name(&self) -> &'static str { "plan.execute" }

    async fn execute(
        &self,
        _with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let plan = require_plan(ctx)?;
        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("plan.execute: no probe data"))?
            .clone();

        // Tonemap requires re-encode. If the flow set plan.video.tonemap but
        // left video.mode = Copy (e.g. a flow that gates plan.video.encode on
        // a non-HEVC check, but adds tonemap unconditionally for HDR), the
        // tonemap intent will be silently dropped — the encoder is never
        // invoked, so no -filter:v fires. Warn loudly so the operator notices.
        if plan.video.tonemap.is_some() && matches!(plan.video.mode, VideoMode::Copy) {
            on_progress(StepProgress::Log(
                "warn: plan.video.tonemap is set but video.mode=copy — tonemap requires re-encode and will not be applied. Place plan.video.tonemap inside the same branch as plan.video.encode.".into()
            ));
        }

        let (src, dest) = staging::next_io(ctx, &plan.container);
        let _ = std::fs::remove_file(&dest);

        let duration_sec = probe
            .get("format")
            .and_then(|f| f.get("duration"))
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        // ---- Hardware acquire (only if video plan asked for HW) -------------
        let mut acquired_key: Option<String> = None;
        let mut hw_permit: Option<tokio::sync::OwnedSemaphorePermit> = None;
        if matches!(plan.video.mode, VideoMode::Encode { .. }) && !plan.video.hw_prefer.is_empty() {
            let prefer: Vec<Accel> = plan
                .video
                .hw_prefer
                .iter()
                .filter_map(|s| Accel::parse(s))
                .collect();
            if let Some((key, permit)) = self.hw.acquire_preferred(&prefer).await {
                acquired_key = Some(key);
                hw_permit = Some(permit);
            } else {
                on_progress(StepProgress::Marker {
                    kind: "hw_unavailable".into(),
                    payload: json!({ "prefer": plan.video.hw_prefer }),
                });
                if !plan.video.hw_fallback_cpu {
                    anyhow::bail!(
                        "plan.execute: no preferred hw accel available and cpu fallback disabled"
                    );
                }
            }
        }

        let cmd = build_command(&src, &dest, &plan, &probe, acquired_key.as_deref(), &self.ffmpeg_caps)?;

        let mut emitted_any_pct = false;
        let result = crate::ffmpeg::run_with_live_events(
            cmd,
            duration_sec,
            ctx.cancel.as_ref(),
            |ev| match ev {
                FfmpegEvent::Pct(p) => {
                    emitted_any_pct = true;
                    on_progress(StepProgress::Pct(p));
                }
                FfmpegEvent::Line(l) => {
                    on_progress(StepProgress::Log(format!("ffmpeg: {l}")));
                }
            },
        )
        .await;

        // Free the GPU permit before any potential CPU fallback.
        drop(hw_permit);

        match result {
            Ok(status) if status.success() => {
                if !emitted_any_pct {
                    on_progress(StepProgress::Pct(100.0));
                }
                staging::record_output(
                    ctx,
                    &dest,
                    json!({ "hw": acquired_key }),
                );
                Ok(())
            }
            Ok(status) => {
                // Ran but exited non-zero. CPU fallback if HW was used.
                if acquired_key.is_some() && plan.video.hw_fallback_cpu {
                    on_progress(StepProgress::Marker {
                        kind: "hw_runtime_failure".into(),
                        payload: json!({ "device": acquired_key }),
                    });
                    let _ = std::fs::remove_file(&dest);
                    let cpu_cmd = build_command(&src, &dest, &plan, &probe, None, &self.ffmpeg_caps)?;
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
            Err(e) => Err(e),
        }
    }
}

/// Build the single ffmpeg invocation that materializes `plan`.
fn build_command(
    src: &std::path::Path,
    dest: &std::path::Path,
    plan: &crate::flow::plan::StreamPlan,
    probe: &Value,
    acquired_key: Option<&str>,
    ffmpeg_caps: &crate::ffmpeg_caps::FfmpegCaps,
) -> anyhow::Result<Command> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-y"]);
    for arg in &plan.global_input_args {
        cmd.arg(arg);
    }
    cmd.arg("-i").arg(src);

    // Map every kept input stream individually so we can apply per-stream codec
    // args. Indices come from probe (the source's stream index).
    let kept = plan.kept_indices();
    if kept.is_empty() {
        anyhow::bail!("plan.execute: plan has no kept streams");
    }

    // First pass: emit -map for each kept input stream.
    for idx in &kept {
        cmd.args(["-map", &format!("0:{idx}")]);
    }

    // Per-output-stream codec args. Output stream indices are assigned in the
    // order they're mapped, separately for each codec_type. Track that here so
    // we can address per-output-type via `-c:v:N`, `-c:a:N`, `-c:s:N`.
    let streams_by_idx = streams_by_index(probe);
    let video_codec_arg = match &plan.video.mode {
        VideoMode::Copy => None,
        VideoMode::Encode { codec } => Some(pick_codec_arg(codec, acquired_key)?),
    };

    let mut v_out = 0i64;
    let mut a_out = 0i64;
    let mut s_out = 0i64;
    let mut d_out = 0i64;

    let force_10bit = plan.video.preserve_10bit && detect_10bit(probe);

    for idx in &kept {
        let s = streams_by_idx
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("plan.execute: kept index {idx} missing in probe"))?;
        let codec_type = s.get("codec_type").and_then(|v| v.as_str()).unwrap_or("");
        match codec_type {
            "video" => {
                if let Some(arg) = video_codec_arg {
                    cmd.args([&format!("-c:v:{v_out}"), arg]);
                    if let Some(crf) = plan.video.crf {
                        cmd.args([&format!("-crf:v:{v_out}"), &crf.to_string()]);
                    }
                    if let Some(preset) = plan.video.preset.as_deref() {
                        cmd.args([&format!("-preset:v:{v_out}"), preset]);
                    }
                    if let Some(tm) = &plan.video.tonemap {
                        // Tonemap to BT.709 SDR. Forces 8-bit yuv420p
                        // output, overriding any preserve_10bit setting
                        // — HDR→SDR fundamentally produces 8-bit.
                        let vf = build_tonemap_vf(tm.engine, ffmpeg_caps.has_libplacebo);
                        cmd.args([&format!("-filter:v:{v_out}"), vf]);
                        cmd.args([&format!("-pix_fmt:v:{v_out}"), "yuv420p"]);
                    } else if force_10bit {
                        cmd.args([&format!("-profile:v:{v_out}"), "main10"]);
                        cmd.args([&format!("-pix_fmt:v:{v_out}"), "p010le"]);
                    }
                } else {
                    cmd.args([&format!("-c:v:{v_out}"), "copy"]);
                }
                v_out += 1;
            }
            "audio" => {
                cmd.args([&format!("-c:a:{a_out}"), "copy"]);
                a_out += 1;
            }
            "subtitle" => {
                cmd.args([&format!("-c:s:{s_out}"), "copy"]);
                s_out += 1;
            }
            "data" => {
                cmd.args([&format!("-c:d:{d_out}"), "copy"]);
                d_out += 1;
            }
            _ => {}
        }
    }

    // Append any added audio streams: each one is a re-encode from a seed input
    // index, with explicit codec/channels/language/title metadata.
    for added in &plan.audio_added {
        cmd.args(["-map", &format!("0:{}", added.seed_index)]);
        let encoder = match added.codec.as_str() {
            "ac3" => "ac3",
            "eac3" => "eac3",
            "aac" => "aac",
            other => anyhow::bail!("plan.execute: unsupported added audio codec {other}"),
        };
        cmd.args([
            &format!("-c:a:{a_out}"),
            encoder,
            &format!("-ac:a:{a_out}"),
            &added.channels.to_string(),
            &format!("-metadata:s:a:{a_out}"),
            &format!("language={}", added.language),
            &format!("-metadata:s:a:{a_out}"),
            &format!("title={}", added.title),
        ]);
        a_out += 1;
    }

    cmd.arg(dest);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    Ok(cmd)
}

fn streams_by_index(probe: &Value) -> std::collections::BTreeMap<i64, Value> {
    let mut out = std::collections::BTreeMap::new();
    if let Some(arr) = probe.get("streams").and_then(|s| s.as_array()) {
        for s in arr {
            if let Some(idx) = s.get("index").and_then(|v| v.as_i64()) {
                out.insert(idx, s.clone());
            }
        }
    }
    out
}

fn detect_10bit(probe: &Value) -> bool {
    let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) else {
        return false;
    };
    for s in streams {
        if s.get("codec_type").and_then(|v| v.as_str()) != Some("video") {
            continue;
        }
        let pix_fmt = s.get("pix_fmt").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        let bps = s.get("bits_per_raw_sample").and_then(|v| v.as_str()).unwrap_or("");
        return bps == "10"
            || pix_fmt.contains("p010")
            || pix_fmt.contains("yuv420p10")
            || pix_fmt.contains("yuv422p10")
            || pix_fmt.contains("yuv444p10");
    }
    false
}

fn pick_codec_arg(codec: &str, acquired_key: Option<&str>) -> anyhow::Result<&'static str> {
    let accel = acquired_key.and_then(|k| k.split(':').next());
    Ok(match (codec, accel) {
        ("x264", Some("nvenc")) => "h264_nvenc",
        ("x265" | "hevc", Some("nvenc")) => "hevc_nvenc",
        ("x264", Some("qsv")) => "h264_qsv",
        ("x265" | "hevc", Some("qsv")) => "hevc_qsv",
        ("x264", Some("vaapi")) => "h264_vaapi",
        ("x265" | "hevc", Some("vaapi")) => "hevc_vaapi",
        ("x264", Some("videotoolbox")) => "h264_videotoolbox",
        ("x265" | "hevc", Some("videotoolbox")) => "hevc_videotoolbox",
        ("x264", _) => "libx264",
        ("x265" | "hevc", _) => "libx265",
        (other, _) => anyhow::bail!("plan.execute: unsupported video codec {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tonemap_vf_libplacebo_uses_libplacebo_filter() {
        let vf = build_tonemap_vf(TonemapEngine::Libplacebo, false);
        assert!(vf.starts_with("libplacebo="), "got {vf}");
    }

    #[test]
    fn build_tonemap_vf_zscale_uses_zscale_chain() {
        let vf = build_tonemap_vf(TonemapEngine::Zscale, true);
        assert!(vf.starts_with("zscale="), "got {vf}");
        assert!(vf.contains("tonemap=hable"), "got {vf}");
    }

    #[test]
    fn build_tonemap_vf_auto_picks_libplacebo_when_present() {
        let vf = build_tonemap_vf(TonemapEngine::Auto, true);
        assert!(vf.starts_with("libplacebo="), "got {vf}");
    }

    #[test]
    fn build_tonemap_vf_auto_falls_back_to_zscale() {
        let vf = build_tonemap_vf(TonemapEngine::Auto, false);
        assert!(vf.starts_with("zscale="), "got {vf}");
    }
}
