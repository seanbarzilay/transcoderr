//! Boot-time probe of ffmpeg-binary capabilities. Currently surfaces
//! whether `libplacebo` is in the filter list — used by the tonemap
//! step to pick between libplacebo (preferred when present) and the
//! software zscale+tonemap chain (always available).

use tokio::process::Command;

#[derive(Debug, Clone, Default)]
pub struct FfmpegCaps {
    pub has_libplacebo: bool,
}

impl FfmpegCaps {
    /// Run `ffmpeg -hide_banner -filters` and parse the output for
    /// `libplacebo`. On any failure (binary not found, non-zero exit,
    /// non-utf8 output) returns the default `FfmpegCaps {
    /// has_libplacebo: false }` — graceful degradation to the zscale
    /// chain, which is always available.
    pub async fn probe() -> Self {
        let out = match Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-filters")
            .output()
            .await
        {
            Ok(o) => o,
            Err(_) => return Self::default(),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        Self {
            has_libplacebo: stdout
                .lines()
                .any(|l| l.split_whitespace().nth(1) == Some("libplacebo")),
        }
    }
}
