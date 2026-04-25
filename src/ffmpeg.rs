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
