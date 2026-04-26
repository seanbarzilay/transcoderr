use super::{Step, StepProgress};
use crate::flow::{staging, Context};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::Stdio;
use tokio::process::Command;

/// Subtitle codecs we keep; everything else is dropped when `drop_unsupported_subs` is on.
/// Excludes `mov_text` — it's MP4-native and the MKV muxer rejects it with
/// "Function not implemented" at header write time. Keep this list in sync
/// with the matching one in `plan_steps.rs` and `audio_ensure.rs`.
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
];

pub struct StripTracksStep;

#[async_trait]
impl Step for StripTracksStep {
    fn name(&self) -> &'static str {
        "strip.tracks"
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let langs = with
            .get("keep_audio_languages")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec!["eng".into()]);
        let remove_cover_art = with
            .get("remove_cover_art")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let drop_unsupported_subs = with
            .get("drop_unsupported_subs")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (src, dest) = staging::next_io(ctx, "mkv");
        let _ = std::fs::remove_file(&dest);

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-y", "-i"]).arg(&src);

        // Video: capital `V` selects video EXCEPT attached_pic when remove_cover_art is on.
        if remove_cover_art {
            cmd.args(["-map", "0:V", "-c:v", "copy"]);
        } else {
            cmd.args(["-map", "0:v", "-c:v", "copy"]);
        }

        for l in &langs {
            cmd.args(["-map", &format!("0:a:m:language:{l}?"), "-c:a", "copy"]);
        }

        // Subtitles: either copy all or only known codecs (selected per-stream).
        if drop_unsupported_subs {
            let probe = ctx.probe.as_ref();
            let mut kept = 0usize;
            if let Some(streams) = probe.and_then(|p| p.get("streams")).and_then(|s| s.as_array()) {
                for s in streams {
                    if s.get("codec_type").and_then(|v| v.as_str()) != Some("subtitle") {
                        continue;
                    }
                    let codec = s
                        .get("codec_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if SUPPORTED_SUB_CODECS.contains(&codec.as_str()) {
                        let idx = s.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
                        if idx >= 0 {
                            cmd.args(["-map", &format!("0:{idx}")]);
                            kept += 1;
                        }
                    } else {
                        on_progress(StepProgress::Log(format!(
                            "dropping unsupported subtitle codec={}",
                            codec
                        )));
                    }
                }
            }
            if kept > 0 {
                cmd.args(["-c:s", "copy"]);
            }
        } else {
            cmd.args(["-map", "0:s?", "-c:s", "copy"]);
        }

        cmd.arg(&dest);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        on_progress(StepProgress::Log(format!(
            "strip tracks: keep audio {langs:?}{}{}",
            if remove_cover_art { " +remove-cover-art" } else { "" },
            if drop_unsupported_subs { " +drop-unsupported-subs" } else { "" },
        )));
        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("strip.tracks ffmpeg failed");
        }
        staging::record_output(ctx, &dest, json!({}));
        Ok(())
    }
}
