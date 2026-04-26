pub mod manifest;
pub mod subprocess;

use manifest::{DiscoveredPlugin, load_from_dir};
use std::path::Path;

pub fn discover(plugins_dir: &Path) -> anyhow::Result<Vec<DiscoveredPlugin>> {
    if !plugins_dir.exists() { return Ok(vec![]); }
    let mut out = vec![];
    for entry in std::fs::read_dir(plugins_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() { continue; }
        if !path.join("manifest.toml").exists() { continue; }
        match load_from_dir(&path) {
            Ok(p) => out.push(p),
            Err(e) => tracing::warn!(?path, error = %e, "skipping invalid plugin"),
        }
    }
    Ok(out)
}
