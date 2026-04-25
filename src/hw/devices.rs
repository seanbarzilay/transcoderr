use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accel {
    Nvenc,
    Qsv,
    Vaapi,
    VideoToolbox,
}

impl Accel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Accel::Nvenc => "nvenc",
            Accel::Qsv => "qsv",
            Accel::Vaapi => "vaapi",
            Accel::VideoToolbox => "videotoolbox",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "nvenc" => Some(Self::Nvenc),
            "qsv" => Some(Self::Qsv),
            "vaapi" => Some(Self::Vaapi),
            "videotoolbox" => Some(Self::VideoToolbox),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub accel: Accel,
    pub index: u32,
    pub name: String,
    pub max_concurrent: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HwCaps {
    pub probed_at: i64,
    pub ffmpeg_version: Option<String>,
    pub devices: Vec<Device>,
    /// Raw list of ffmpeg-known encoders that match an accel.
    pub encoders: Vec<String>,
}
