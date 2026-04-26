use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bind: String,
    pub data_dir: PathBuf,
    pub radarr: RadarrConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RadarrConfig {
    pub bearer_token: String,
}

impl Config {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&raw)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn parses_minimal_config() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, r#"
bind = "127.0.0.1:8080"
data_dir = "/tmp/tcr"
[radarr]
bearer_token = "abc123"
        "#).unwrap();
        let cfg = Config::from_path(f.path()).unwrap();
        assert_eq!(cfg.bind, "127.0.0.1:8080");
        assert_eq!(cfg.radarr.bearer_token, "abc123");
    }
}
