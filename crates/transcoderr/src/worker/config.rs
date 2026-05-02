//! Worker daemon config. TOML at the path passed to
//! `transcoderr worker --config <path>`.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfig {
    /// Where to dial. Use `wss://` for TLS, `ws://` for plaintext.
    pub coordinator_url: String,
    /// The token minted in the coordinator's UI.
    pub coordinator_token: String,
    /// Optional friendly name for the Workers UI. Defaults to hostname.
    pub name: Option<String>,
}

impl WorkerConfig {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("read {}: {e}", path.display()))?;
        toml::from_str(&s).map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))
    }

    pub fn resolved_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unnamed-worker".into())
        })
    }
}
