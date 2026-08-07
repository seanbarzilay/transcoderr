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

/// The `tags.language` of a probe stream, verbatim. Empty when the stream
/// carries no language tag — which is what `0:a:m:language:X:?` fails to
/// match, and why the caller has to check.
///
/// Deliberately not normalised: ffmpeg's `m:language:X` is a plain string
/// comparison, so a prediction that folds case would claim a match ffmpeg
/// never makes, skip the fallback below, and emit exactly the audio-less
/// file the caller is trying to avoid.
fn stream_language(s: &Value) -> &str {
    s.get("tags")
        .and_then(|t| t.get("language"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
}

pub struct StripTracksStep;

#[async_trait]
impl Step for StripTracksStep {
    fn name(&self) -> &'static str {
        "strip.tracks"
    }

    fn executor(&self) -> crate::steps::Executor {
        crate::steps::Executor::Any
    }

    fn with_schema(&self) -> Option<Value> {
        Some(super::schemas::strip_tracks_schema())
    }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        // `languages` is the documented key (see StripTracksConfig, which
        // is `deny_unknown_fields`); `keep_audio_languages` is what this
        // step originally read. Accept both so neither a flow written
        // against the published schema nor an older one silently falls
        // back to the default.
        let langs = with
            .get("languages")
            .or_else(|| with.get("keep_audio_languages"))
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
            // ffmpeg 7.1 tightened stream-specifier parsing: the `?`
            // (optional) qualifier must be its own colon-separated
            // component when combined with metadata matching. Old form
            // `m:language:eng?` errors on 7.1+; canonical `:?` form
            // works on 6.x as well.
            cmd.args(["-map", &format!("0:a:m:language:{l}:?"), "-c:a", "copy"]);
        }

        // That `:?` is also a trap: a selector matching nothing is a no-op
        // rather than an error, so a language filter that matches no
        // stream produces an output with zero audio streams and exit
        // status 0. This step would then publish it as the chain head and
        // a downstream `output: replace` would rename it over the user's
        // only copy — video and subtitles intact, audio gone, run
        // reported `completed`. Predict the selection and refuse instead.
        let audio_streams: Vec<&Value> = ctx
            .probe
            .as_ref()
            .and_then(|p| p.get("streams"))
            .and_then(|s| s.as_array())
            .map(|streams| {
                streams
                    .iter()
                    .filter(|s| s.get("codec_type").and_then(|v| v.as_str()) == Some("audio"))
                    .collect()
            })
            .unwrap_or_default();

        let matched = audio_streams
            .iter()
            .filter(|s| langs.iter().any(|l| l.as_str() == stream_language(s)))
            .count();

        // `ctx.probe` describes `ctx.file.path`, but ffmpeg reads `src` —
        // the chain head once an earlier transformer has staged a file.
        // Only `steps/probe.rs` writes `ctx.probe` and no transformer
        // refreshes it, so mid-chain it is stale: in
        // `probe -> audio.ensure(target_lang: eng) -> strip.tracks([eng])`
        // over a jpn-only source, audio.ensure adds the very track being
        // asked for, yet the probe still shows jpn only. Predicting from
        // that would fail a flow that works. Predict only when the probe
        // describes the file ffmpeg will actually read.
        //
        // With no probe data there is nothing to predict from either;
        // leave the command as built rather than guess.
        let probe_describes_input = src.as_path() == std::path::Path::new(&ctx.file.path);
        if probe_describes_input && !audio_streams.is_empty() && matched == 0 {
            // Untagged and `und` streams match no language selector, and
            // they are the common case in remuxes — including files this
            // tool produced itself. Keep them rather than emit a silent
            // file: we cannot prove they are not what was asked for.
            let untagged: Vec<i64> = audio_streams
                .iter()
                .filter(|s| {
                    let l = stream_language(s);
                    l.is_empty() || l.eq_ignore_ascii_case("und")
                })
                .filter_map(|s| s.get("index").and_then(|v| v.as_i64()))
                .collect();

            if untagged.is_empty() {
                let present: Vec<&str> = audio_streams.iter().map(|s| stream_language(s)).collect();
                anyhow::bail!(
                    "strip.tracks: no audio stream matches {langs:?} (present: {present:?}); \
                     refusing to write a file with no audio"
                );
            }

            on_progress(StepProgress::Log(format!(
                "no audio stream tagged {langs:?}; keeping {} untagged stream(s)",
                untagged.len()
            )));
            for idx in untagged {
                cmd.args(["-map", &format!("0:{idx}"), "-c:a", "copy"]);
            }
        }

        // Subtitles: either copy all or only known codecs (selected per-stream).
        if drop_unsupported_subs {
            let probe = ctx.probe.as_ref();
            let mut kept = 0usize;
            if let Some(streams) = probe
                .and_then(|p| p.get("streams"))
                .and_then(|s| s.as_array())
            {
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
            if remove_cover_art {
                " +remove-cover-art"
            } else {
                ""
            },
            if drop_unsupported_subs {
                " +drop-unsupported-subs"
            } else {
                ""
            },
        )));
        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("strip.tracks ffmpeg failed");
        }
        staging::record_output(ctx, &dest, json!({}));
        Ok(())
    }
}
