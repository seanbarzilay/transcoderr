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
    /// Bare executable names the plugin shells out to (e.g.
    /// `["python3"]`, `["node"]`). The server checks each is on `$PATH`
    /// before allowing install. Empty / omitted means "POSIX shell +
    /// coreutils only" — always present on supported images.
    #[serde(default)]
    pub runtimes: Vec<String>,
    /// Optional shell command run by `/bin/sh -c` in the plugin's
    /// directory at install time and on every server boot. Used by
    /// authors of e.g. Python plugins to declare `pip install -r
    /// requirements.txt`. Failure at install returns 422 and rolls
    /// back the install; failure at boot logs a warning and the
    /// plugin still registers (so the operator can see it in the UI
    /// to debug).
    #[serde(default)]
    pub deps: Option<String>,
    /// Per-step routing overrides. Each key is a step kind from
    /// `provides_steps`. Steps with no entry default to
    /// `coordinator-only`. See spec/distributed-piece-5.
    #[serde(default)]
    pub steps: std::collections::BTreeMap<String, StepManifest>,
}

/// Per-step manifest entry. Lives in the `[steps."<step_kind>"]`
/// table inside `manifest.toml`. The only field today is `executor`,
/// which defaults to coordinator-only when omitted.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StepManifest {
    #[serde(default)]
    pub executor: Option<ManifestExecutor>,
}

/// Wire / TOML form of `crate::steps::Executor`. Kebab-case for TOML
/// readability (`any-worker` matches the spec's prose better than
/// `any_worker`).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManifestExecutor {
    AnyWorker,
    CoordinatorOnly,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialise_with_steps_block() {
        let toml_src = r#"
name = "whisper"
version = "1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["whisper.transcribe", "whisper.detect_language"]

[steps."whisper.transcribe"]
executor = "any-worker"
"#;
        let m: Manifest = toml::from_str(toml_src).unwrap();
        assert_eq!(m.steps.len(), 1);
        let entry = m.steps.get("whisper.transcribe").unwrap();
        assert_eq!(entry.executor, Some(ManifestExecutor::AnyWorker));
        // The other declared step has no [steps.X] entry, so it's absent
        // from the map — the registry build path defaults to
        // CoordinatorOnly when looking up a missing key.
        assert!(m.steps.get("whisper.detect_language").is_none());
    }

    #[test]
    fn deserialise_without_steps_block() {
        // Existing manifest shape (size-report) still parses cleanly;
        // `steps` defaults to an empty map.
        let toml_src = r#"
name = "size-report"
version = "0.1.2"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["size.report"]
"#;
        let m: Manifest = toml::from_str(toml_src).unwrap();
        assert!(m.steps.is_empty(), "missing [steps] block → empty map");
    }
}
