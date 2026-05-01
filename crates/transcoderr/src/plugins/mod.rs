pub mod catalog;
pub mod deps;
pub mod installer;
pub mod manifest;
pub mod runtime;
pub mod subprocess;
pub mod uninstaller;

use manifest::{DiscoveredPlugin, load_from_dir};
use std::path::Path;

pub fn discover(plugins_dir: &Path) -> anyhow::Result<Vec<DiscoveredPlugin>> {
    if !plugins_dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(plugins_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() { continue; }
        let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if dir_name.starts_with(".tcr-") {
            continue;
        }
        if !path.join("manifest.toml").exists() { continue; }
        match load_from_dir(&path) {
            Ok(p) => out.push(p),
            Err(e) => tracing::warn!(?path, error = %e, "skipping invalid plugin"),
        }
    }
    Ok(out)
}
