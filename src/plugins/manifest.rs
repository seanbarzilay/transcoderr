use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub kind: String,                // "subprocess" or "builtin"
    pub entrypoint: Option<String>,  // required for subprocess
    pub provides_steps: Vec<String>,
    #[serde(default)]
    pub requires: serde_json::Value,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub manifest: Manifest,
    pub manifest_dir: PathBuf,
    pub schema: serde_json::Value,
}

pub fn load_from_dir(dir: &Path) -> anyhow::Result<DiscoveredPlugin> {
    let manifest_path = dir.join("manifest.toml");
    let raw = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&raw)?;
    let schema_path = dir.join("schema.json");
    let schema: serde_json::Value = if schema_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&schema_path)?)?
    } else {
        serde_json::json!({})
    };
    Ok(DiscoveredPlugin { manifest, manifest_dir: dir.to_path_buf(), schema })
}
