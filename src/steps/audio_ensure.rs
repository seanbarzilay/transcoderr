use super::{Step, StepProgress};
use crate::flow::Context;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Audio codecs that can be passed through to most consumer players without re-encoding.
const PLAYABLE_AUDIO: &[&str] = &["aac", "ac3", "eac3", "mp3", "opus"];

/// Subtitle codecs that survive standard mkv muxing.
const SUPPORTED_SUB_CODECS: &[&str] = &[
    "srt",
    "subrip",
    "ass",
    "ssa",
    "mov_text",
    "hdmv_pgs_subtitle",
    "pgssub",
    "dvd_subtitle",
    "dvdsub",
    "dvb_subtitle",
];

/// `audio.ensure` — bundles the per-stream housekeeping that a tdarr "ensureAudio + mark
/// unsupported subs + dedupe added audio + remove attached_pic" chain does, in a single
/// ffmpeg invocation.
///
/// Reads probe data on the context, builds a stream-map plan, and runs one ffmpeg pass
/// that:
/// - Copies every video stream (skipping attached_pic when `drop_cover_art` is on)
/// - Copies every non-commentary audio stream
/// - Optionally adds a transcoded audio stream of the configured `codec`/`language`/`channels`
///   if no existing playable stream already meets the spec
/// - Drops or keeps subtitle streams based on `drop_unsupported_subs`
/// - Drops data streams when `drop_data_streams` is on
///
/// Output goes to `<src>.transcoderr.tmp.mkv` and the path is recorded under
/// `ctx.steps["transcode"]["output_path"]` so a downstream `output: replace` step picks it up.
pub struct AudioEnsureStep;

#[derive(Debug)]
struct StreamInfo {
    index: i64,
    codec_type: String,
    codec_name: String,
    channels: i64,
    language: String,
    is_commentary: bool,
    is_attached_pic: bool,
}

fn parse_streams(probe: &Value) -> Vec<StreamInfo> {
    let mut out = vec![];
    let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) else {
        return out;
    };
    for s in streams {
        let index = s.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
        let codec_type = s.get("codec_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let codec_name = s
            .get("codec_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let channels = s.get("channels").and_then(|v| v.as_i64()).unwrap_or(0);
        let language = s
            .get("tags")
            .and_then(|t| t.get("language"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let title = s
            .get("tags")
            .and_then(|t| t.get("title"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let comment_disp = s
            .get("disposition")
            .and_then(|d| d.get("comment"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            == 1;
        let is_commentary = codec_type == "audio" && (comment_disp || title.contains("comment"));
        let is_attached_pic = s
            .get("disposition")
            .and_then(|d| d.get("attached_pic"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            == 1;
        out.push(StreamInfo {
            index,
            codec_type,
            codec_name,
            channels,
            language,
            is_commentary,
            is_attached_pic,
        });
    }
    out
}

#[async_trait]
impl Step for AudioEnsureStep {
    fn name(&self) -> &'static str {
        "audio.ensure"
    }

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
            .unwrap_or(6) as i64;
        let drop_cover_art = with
            .get("drop_cover_art")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let drop_unsupported_subs = with
            .get("drop_unsupported_subs")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let drop_data_streams = with
            .get("drop_data_streams")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let tolerate_errors = with
            .get("tolerate_errors")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let probe = ctx
            .probe
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("audio.ensure: no probe data on context (run probe first)"))?;
        let streams = parse_streams(probe);
        if streams.is_empty() {
            anyhow::bail!("audio.ensure: probe reported zero streams");
        }

        // Decide whether the target audio already exists.
        let has_target = streams.iter().any(|s| {
            s.codec_type == "audio"
                && !s.is_commentary
                && s.codec_name == target_codec
                && s.channels >= target_channels
                && (s.language == target_lang
                    || s.language.is_empty()
                    || s.language == "und")
        });

        // Find the highest-channel non-commentary audio source — used as the seed for the
        // added stream when we need to ensure it.
        let seed_audio = streams
            .iter()
            .filter(|s| s.codec_type == "audio" && !s.is_commentary)
            .max_by_key(|s| s.channels)
            .map(|s| (s.index, s.channels));

        // Existing playable max channels (used for the dedupe rule).
        let playable_max_ch = streams
            .iter()
            .filter(|s| {
                s.codec_type == "audio"
                    && !s.is_commentary
                    && PLAYABLE_AUDIO.contains(&s.codec_name.as_str())
            })
            .map(|s| s.channels)
            .max()
            .unwrap_or(0);

        let mut add_stream = if !has_target {
            seed_audio
        } else {
            on_progress(StepProgress::Log(format!(
                "audio.ensure: existing {target_codec} {target_channels}ch [{target_lang}] already present; skipping add"
            )));
            None
        };

        // Dedupe: drop the addition if it wouldn't add anything beyond what the source already has.
        if let Some((_, _seed_ch)) = add_stream {
            if target_channels <= playable_max_ch {
                on_progress(StepProgress::Log(format!(
                    "audio.ensure: skipping add (existing playable {playable_max_ch}ch already >= target {target_channels}ch)"
                )));
                add_stream = None;
            }
        }

        // Build the ffmpeg command.
        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-y"]);
        if tolerate_errors {
            cmd.args(["-err_detect", "ignore_err", "-fflags", "+discardcorrupt"]);
        }
        cmd.arg("-i").arg(&src);

        let mut audio_out_idx = 0i64;
        let mut sub_out_idx = 0i64;
        let mut new_audio_out_idx: Option<i64> = None;

        // Map streams in order.
        for s in &streams {
            match s.codec_type.as_str() {
                "video" => {
                    if drop_cover_art && s.is_attached_pic {
                        on_progress(StepProgress::Log(format!(
                            "audio.ensure: drop cover-art stream {}",
                            s.index
                        )));
                        continue;
                    }
                    cmd.args(["-map", &format!("0:{}", s.index)]);
                }
                "audio" => {
                    cmd.args(["-map", &format!("0:{}", s.index)]);
                    cmd.args([
                        &format!("-c:a:{audio_out_idx}"),
                        "copy",
                    ]);
                    audio_out_idx += 1;
                }
                "subtitle" => {
                    if drop_unsupported_subs
                        && !SUPPORTED_SUB_CODECS.contains(&s.codec_name.as_str())
                    {
                        on_progress(StepProgress::Log(format!(
                            "audio.ensure: drop unsupported subtitle {} (codec={})",
                            s.index, s.codec_name
                        )));
                        continue;
                    }
                    cmd.args(["-map", &format!("0:{}", s.index)]);
                    cmd.args([&format!("-c:s:{sub_out_idx}"), "copy"]);
                    sub_out_idx += 1;
                }
                "data" => {
                    if drop_data_streams {
                        on_progress(StepProgress::Log(format!(
                            "audio.ensure: drop data stream {}",
                            s.index
                        )));
                        continue;
                    }
                    cmd.args(["-map", &format!("0:{}", s.index)]);
                }
                _ => {}
            }
        }

        // Add the ensured audio stream, if needed.
        if let Some((seed_index, _)) = add_stream {
            let encoder = match target_codec.as_str() {
                "ac3" => "ac3",
                "eac3" => "eac3",
                "aac" => "aac",
                other => anyhow::bail!("audio.ensure: unsupported target codec {other}"),
            };
            cmd.args(["-map", &format!("0:{seed_index}")]);
            cmd.args([
                &format!("-c:a:{audio_out_idx}"),
                encoder,
                &format!("-ac:a:{audio_out_idx}"),
                &target_channels.to_string(),
                &format!("-metadata:s:a:{audio_out_idx}"),
                &format!("language={target_lang}"),
            ]);
            new_audio_out_idx = Some(audio_out_idx);
            on_progress(StepProgress::Log(format!(
                "audio.ensure: adding {target_codec} {target_channels}ch [{target_lang}] from source stream {seed_index}"
            )));
        }

        cmd.arg(&dest);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("audio.ensure: ffmpeg failed");
        }

        let mut out = json!({ "output_path": dest.to_string_lossy() });
        if let Some(idx) = new_audio_out_idx {
            out["added_audio_index"] = json!(idx);
        }
        ctx.record_step_output("transcode", out);
        Ok(())
    }
}
