use super::devices::{Accel, Device, HwCaps};
use std::process::Stdio;
use tokio::process::Command;

const HW_ENCODERS: &[(&str, Accel)] = &[
    ("h264_nvenc", Accel::Nvenc),
    ("hevc_nvenc", Accel::Nvenc),
    ("av1_nvenc", Accel::Nvenc),
    ("h264_qsv", Accel::Qsv),
    ("hevc_qsv", Accel::Qsv),
    ("h264_vaapi", Accel::Vaapi),
    ("hevc_vaapi", Accel::Vaapi),
    ("h264_videotoolbox", Accel::VideoToolbox),
    ("hevc_videotoolbox", Accel::VideoToolbox),
];

pub async fn probe() -> HwCaps {
    let mut caps = HwCaps {
        probed_at: chrono::Utc::now().timestamp(),
        ..Default::default()
    };

    // Get ffmpeg version string.
    if let Ok(o) = Command::new("ffmpeg")
        .arg("-version")
        .stderr(Stdio::null())
        .output()
        .await
    {
        if let Some(line) = String::from_utf8_lossy(&o.stdout).lines().next() {
            caps.ffmpeg_version = Some(line.to_string());
        }
    }

    // Probe hardware encoders.
    if let Ok(o) = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stderr(Stdio::null())
        .output()
        .await
    {
        let s = String::from_utf8_lossy(&o.stdout).to_string();
        let mut found = vec![];
        // Deduplicate by accel: only add one device per accel type.
        let mut seen_accels: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (name, accel) in HW_ENCODERS {
            if s.contains(name) {
                found.push(name.to_string());
                let accel_key = accel.as_str().to_string();
                if seen_accels.insert(accel_key) {
                    caps.devices.push(Device {
                        accel: accel.clone(),
                        index: 0,
                        name: format!("{} (default)", name),
                        max_concurrent: default_concurrency(accel),
                    });
                }
            }
        }
        caps.encoders = found;
    }

    // Refine NVENC device count using nvidia-smi if available.
    if caps.devices.iter().any(|d| d.accel == Accel::Nvenc) {
        if let Ok(o) = Command::new("nvidia-smi")
            .args(["-L"])
            .stderr(Stdio::null())
            .output()
            .await
        {
            let listing = String::from_utf8_lossy(&o.stdout);
            let n = listing.lines().filter(|l| l.starts_with("GPU ")).count() as u32;
            if n > 0 {
                // Replace the placeholder NVENC device with one per detected GPU.
                caps.devices.retain(|d| d.accel != Accel::Nvenc);
                for i in 0..n {
                    caps.devices.push(Device {
                        accel: Accel::Nvenc,
                        index: i,
                        name: format!("NVENC GPU{}", i),
                        max_concurrent: 3,
                    });
                }
            }
        }
    }

    caps
}

fn default_concurrency(accel: &Accel) -> u32 {
    match accel {
        Accel::Nvenc => 3, // consumer-card session limit
        Accel::Qsv => 8,
        Accel::Vaapi => 8,
        Accel::VideoToolbox => 4,
    }
}

/// Sync helper for testing: parse an `ffmpeg -encoders` stdout listing
/// and return the names of known HW encoders found.
pub fn parse_encoders_listing(stdout: &str) -> Vec<&'static str> {
    HW_ENCODERS
        .iter()
        .filter(|(n, _)| stdout.contains(*n))
        .map(|(n, _)| *n)
        .collect()
}
