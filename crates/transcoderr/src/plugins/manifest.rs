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
    /// One-line description shown in the Plugins detail panel and the
    /// catalog Browse list. Plugin authors set this in manifest.toml;
    /// the catalog repo's publish.py copies it into index.json.
    #[serde(default)]
    pub summary: Option<String>,
    /// Minimum transcoderr version this plugin is known to work
    /// against. The catalog Browse tab uses it to gate Install on a
    /// stale server; for already-installed plugins it's just a label
    /// in the detail panel.
    #[serde(default)]
    pub min_transcoderr_version: Option<String>,
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
