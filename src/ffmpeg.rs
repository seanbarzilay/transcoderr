use anyhow::Context;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub async fn ffprobe_json(path: &Path) -> anyhow::Result<Value> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output().await
        .context("spawn ffprobe")?;
    if !out.status.success() {
        anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    Ok(v)
}

/// Generate a tiny test mkv at `dest`. Returns Ok(()) on success.
/// Used only by integration tests.
pub async fn make_testsrc_mkv(dest: &Path, seconds: u32) -> anyhow::Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi", "-i", &format!("testsrc=duration={seconds}:size=320x240:rate=30"),
            "-f", "lavfi", "-i", &format!("sine=duration={seconds}:frequency=440"),
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
            "-c:a", "aac",
            "-shortest",
        ])
        .arg(dest)
        .status().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg testsrc generation failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn probes_a_generated_clip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("t.mkv");
        make_testsrc_mkv(&p, 1).await.unwrap();
        let v = ffprobe_json(&p).await.unwrap();
        let streams = v["streams"].as_array().unwrap();
        assert!(streams.iter().any(|s| s["codec_type"] == "video"));
    }
}

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;

/// Parses lines like `frame=  120 fps= 30 q=28.0 ... time=00:00:04.00 ... speed=1.0x`
/// into approximate progress percent given total `duration_sec`.
pub struct ProgressParser {
    pub duration_sec: f64,
}

impl ProgressParser {
    pub fn parse_line(&self, line: &str) -> Option<f64> {
        let time_idx = line.find("time=")? + 5;
        let time_str = &line[time_idx..];
        let end = time_str.find(' ').unwrap_or(time_str.len());
        let t = parse_hhmmss(&time_str[..end])?;
        if self.duration_sec <= 0.0 { return None; }
        Some((t / self.duration_sec * 100.0).clamp(0.0, 100.0))
    }
}

fn parse_hhmmss(s: &str) -> Option<f64> {
    let mut parts = s.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Stream events emitted while draining ffmpeg's stderr.
#[derive(Debug, Clone)]
pub enum FfmpegEvent {
    Pct(f64),
    Line(String),
}

/// Spawn ffmpeg and drain its stderr LIVE, emitting `FfmpegEvent::Pct` (when the
/// progress percentage advances by ≥1) and `FfmpegEvent::Line` (throttled to one
/// progress line every ~1.5s) as the encode runs. Callers receive events while
/// the child process is still running, which is what makes the run-detail page
/// show live progress + the latest ffmpeg line in real time.
pub async fn run_with_live_events<F>(
    mut cmd: tokio::process::Command,
    duration_sec: f64,
    cancel: Option<&tokio_util::sync::CancellationToken>,
    mut on_event: F,
) -> anyhow::Result<std::process::ExitStatus>
where
    F: FnMut(FfmpegEvent),
{
    use std::process::Stdio;
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let stderr = child.stderr.take().expect("piped");
    let parser = ProgressParser { duration_sec };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FfmpegEvent>();
    let drain_handle = tokio::spawn(async move {
        drain_stderr(stderr, parser, move |ev| {
            let _ = tx.send(ev);
        })
        .await;
    });

    // Spawn a separate killer task that sends SIGKILL to the child's PID when
    // cancellation fires. We can't call child.start_kill() directly inside the
    // select! loop because child.wait() already holds a mutable borrow.
    let pid = child.id();
    let cancel_for_killer = cancel.cloned();
    let killer_handle = tokio::spawn(async move {
        let (Some(token), Some(p)) = (cancel_for_killer, pid) else { return };
        token.cancelled().await;
        let _ = tokio::process::Command::new("kill")
            .arg("-KILL")
            .arg(p.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    });

    let mut waiter = Box::pin(child.wait());
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
    tick.tick().await;

    let mut last_pct = 0.0_f64;
    let mut last_line_at: Option<std::time::Instant> = None;
    let mut throttle = |ev: FfmpegEvent, on_event: &mut F| match ev {
        FfmpegEvent::Pct(pct) => {
            if pct - last_pct >= 1.0 {
                last_pct = pct;
                on_event(FfmpegEvent::Pct(pct));
            }
        }
        FfmpegEvent::Line(line) => {
            let is_progress = line.contains("time=") && line.contains("speed=");
            if !is_progress {
                return;
            }
            let now = std::time::Instant::now();
            if last_line_at.map_or(true, |t| now.duration_since(t).as_millis() >= 1500) {
                last_line_at = Some(now);
                on_event(FfmpegEvent::Line(line.trim().to_string()));
            }
        }
    };

    let status_result = loop {
        tokio::select! {
            biased;
            _ = tick.tick() => {
                while let Ok(ev) = rx.try_recv() {
                    throttle(ev, &mut on_event);
                }
            }
            res = &mut waiter => break res,
        }
    };

    killer_handle.abort();
    let _ = drain_handle.await;
    while let Ok(ev) = rx.try_recv() {
        throttle(ev, &mut on_event);
    }

    if cancel.map(|t| t.is_cancelled()).unwrap_or(false) {
        anyhow::bail!("cancelled");
    }
    Ok(status_result?)
}

/// Drains ffmpeg's stderr line by line. For each ffmpeg progress line it emits a
/// `Pct` event (parsed from `time=`); the same line is also forwarded as `Line`
/// so callers can surface live ffmpeg output in the UI. Non-progress lines
/// (warnings, errors, codec info) are emitted as `Line` only.
pub async fn drain_stderr<F>(stderr: ChildStderr, parser: ProgressParser, mut on_event: F)
where
    F: FnMut(FfmpegEvent),
{
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if let Some(pct) = parser.parse_line(&line) {
            on_event(FfmpegEvent::Pct(pct));
        }
        on_event(FfmpegEvent::Line(line));
    }
}

pub async fn drain_stderr_progress<F>(stderr: ChildStderr, parser: ProgressParser, mut on_pct: F)
where F: FnMut(f64) {
    let mut reader = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if let Some(pct) = parser.parse_line(&line) {
            on_pct(pct);
        }
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;
    #[test]
    fn parses_progress_line() {
        let p = ProgressParser { duration_sec: 100.0 };
        let pct = p.parse_line("frame=  120 fps= 30 q=28.0 size=N/A time=00:00:50.00 bitrate=N/A speed=1.0x").unwrap();
        assert!((pct - 50.0).abs() < 0.001);
    }
}
