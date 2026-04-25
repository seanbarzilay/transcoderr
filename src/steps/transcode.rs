use super::{Step, StepProgress};
use crate::ffmpeg::{drain_stderr_progress, ProgressParser};
use crate::flow::Context;
use crate::hw::{devices::Accel, semaphores::DeviceRegistry};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

pub struct TranscodeStep {
    pub hw: DeviceRegistry,
}

#[async_trait]
impl Step for TranscodeStep {
    fn name(&self) -> &'static str { "transcode" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut Context,
        on_progress: &mut (dyn FnMut(StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let codec = with.get("codec").and_then(|v| v.as_str()).unwrap_or("x265");
        let crf = with.get("crf").and_then(|v| v.as_i64()).unwrap_or(22);
        let preset = with.get("preset").and_then(|v| v.as_str()).unwrap_or("medium");

        // Parse hw block.
        let hw_block = with.get("hw").cloned().unwrap_or(Value::Null);
        let prefer: Vec<Accel> = hw_block
            .get("prefer")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().and_then(Accel::parse))
                    .collect()
            })
            .unwrap_or_default();
        let cpu_fallback =
            hw_block.get("fallback").and_then(|v| v.as_str()) == Some("cpu");

        let src = Path::new(&ctx.file.path).to_path_buf();
        let dest = src.with_extension("transcoderr.tmp.mkv");
        let _ = std::fs::remove_file(&dest);

        let duration_sec = ctx
            .probe
            .as_ref()
            .and_then(|p| p["format"]["duration"].as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        // Acquire a GPU permit if requested.
        let mut acquired_key: Option<String> = None;
        let mut hw_permit: Option<tokio::sync::OwnedSemaphorePermit> = None;
        if !prefer.is_empty() {
            if let Some((key, permit)) = self.hw.acquire_preferred(&prefer).await {
                acquired_key = Some(key);
                hw_permit = Some(permit);
            } else {
                on_progress(StepProgress::Marker {
                    kind: "hw_unavailable".into(),
                    payload: json!({
                        "prefer": prefer.iter().map(|a| a.as_str()).collect::<Vec<_>>()
                    }),
                });
                if !cpu_fallback {
                    anyhow::bail!(
                        "no preferred hw accel available and cpu fallback disabled"
                    );
                }
            }
        }

        let codec_arg = pick_codec_arg(codec, acquired_key.as_deref())?;

        // First attempt.
        let result = run_ffmpeg(
            &src,
            &dest,
            codec_arg,
            preset,
            crf,
            duration_sec,
            on_progress,
        )
        .await;

        // Drop GPU permit before any fallback so the slot is freed immediately.
        drop(hw_permit);

        match result {
            Ok(()) => {
                ctx.record_step_output(
                    "transcode",
                    json!({
                        "output_path": dest.to_string_lossy(),
                        "codec": codec,
                        "hw": acquired_key,
                    }),
                );
                Ok(())
            }
            Err(e) => {
                if is_disk_full(&e) {
                    anyhow::bail!("disk_full");
                }
                if cpu_fallback && acquired_key.is_some() {
                    on_progress(StepProgress::Marker {
                        kind: "hw_runtime_failure".into(),
                        payload: json!({
                            "device": acquired_key,
                            "error": e.to_string(),
                        }),
                    });
                    let cpu_codec = match codec {
                        "x264" => "libx264",
                        _ => "libx265",
                    };
                    run_ffmpeg(
                        &src,
                        &dest,
                        cpu_codec,
                        "ultrafast",
                        crf,
                        duration_sec,
                        on_progress,
                    )
                    .await?;
                    ctx.record_step_output(
                        "transcode",
                        json!({
                            "output_path": dest.to_string_lossy(),
                            "codec": codec,
                            "hw": null,
                            "fallback_from": acquired_key,
                        }),
                    );
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }
}

fn pick_codec_arg(codec: &str, acquired_key: Option<&str>) -> anyhow::Result<&'static str> {
    Ok(
        match (
            codec,
            acquired_key.map(|k| k.split(':').next().unwrap_or("")),
        ) {
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
            (other, _) => anyhow::bail!("unsupported codec {other}"),
        },
    )
}

fn is_disk_full(e: &anyhow::Error) -> bool {
    let s = e.to_string().to_lowercase();
    s.contains("no space left") || s.contains("enospc")
}

async fn run_ffmpeg(
    src: &Path,
    dest: &Path,
    codec_arg: &str,
    preset: &str,
    crf: i64,
    duration_sec: f64,
    on_progress: &mut (dyn FnMut(StepProgress) + Send),
) -> anyhow::Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-y", "-i"])
        .arg(src)
        .args([
            "-c:v",
            codec_arg,
            "-preset",
            preset,
            "-crf",
            &crf.to_string(),
            "-c:a",
            "copy",
            "-c:s",
            "copy",
        ])
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let parser = ProgressParser { duration_sec };

    let parse_task = tokio::spawn(async move {
        let mut last = 0.0;
        let mut buf: Vec<f64> = vec![];
        drain_stderr_progress(stderr, parser, |pct| {
            if pct - last >= 1.0 {
                last = pct;
                buf.push(pct);
            }
        })
        .await;
        buf
    });

    let status = child.wait().await?;
    let mut pcts = parse_task.await.unwrap_or_default();

    // Emit a 100% sentinel if no progress was captured (e.g. unknown duration).
    if pcts.is_empty() {
        pcts.push(100.0);
    }

    for p in pcts {
        on_progress(StepProgress::Pct(p));
    }

    if !status.success() {
        anyhow::bail!("ffmpeg exit {:?}", status.code());
    }
    Ok(())
}
