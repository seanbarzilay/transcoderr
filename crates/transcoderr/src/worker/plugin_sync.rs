//! Worker-side plugin synchronisation. Called on `register_ack` and
//! on every `PluginSync` envelope. Mirrors the coordinator's manifest:
//! installs missing plugins, uninstalls anything the coordinator no
//! longer wants. After the sync, the worker's step registry is
//! rebuilt from the new on-disk discovery.

use crate::plugins::catalog::IndexEntry;
use crate::plugins::{installer, uninstaller};
use crate::worker::protocol::PluginInstall;
use std::path::Path;

/// Output of `compute_diff`. Vectors are exclusive — anything in
/// `to_install` is not in `to_remove` and vice versa.
#[derive(Debug, PartialEq)]
pub struct Diff {
    /// Manifest entries that need to be installed (or replaced — a
    /// version bump shows up here too because the installer's
    /// atomic-swap path overwrites the existing dir).
    pub to_install: Vec<PluginInstall>,
    /// Plugin names currently installed but absent from the manifest.
    pub to_remove: Vec<String>,
}

/// Compute the install/remove plan from the current installed set
/// and the coordinator's intended manifest.
///
/// `installed` is `(name, sha256_or_none)`. `sha256_or_none == None`
/// means we don't know the local sha (e.g. a side-loaded plugin that
/// never went through `install_from_entry`); we treat such entries as
/// "needs replace" if the manifest mentions the name.
pub fn compute_diff(
    installed: &[(String, Option<String>)],
    manifest: &[PluginInstall],
) -> Diff {
    let manifest_names: std::collections::HashSet<&str> =
        manifest.iter().map(|m| m.name.as_str()).collect();

    let to_remove: Vec<String> = installed
        .iter()
        .filter(|(name, _)| !manifest_names.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();

    let to_install: Vec<PluginInstall> = manifest
        .iter()
        .filter(|m| {
            // Already installed with matching sha → skip.
            !installed.iter().any(|(name, sha)| {
                name == &m.name && sha.as_deref() == Some(m.sha256.as_str())
            })
        })
        .cloned()
        .collect();

    Diff { to_install, to_remove }
}

/// Run a full mirror sync against the coordinator's manifest.
///
/// Best-effort: every install/uninstall is wrapped so a single
/// failure logs and continues with the rest. The caller cannot
/// distinguish "everything succeeded" from "something failed" —
/// failures land in the worker's logs, and the next `PluginSync`
/// or reconnect retries.
pub async fn sync(
    plugins_dir: &Path,
    manifest: Vec<PluginInstall>,
    coordinator_token: &str,
) {
    // 1. Discover currently-installed plugins.
    let installed = match crate::plugins::discover(plugins_dir) {
        Ok(d) => d
            .into_iter()
            .map(|p| {
                // Read the optional .tcr-sha256 marker file. We write
                // this on successful install (Step 4 below) so the
                // worker can answer "what sha was this installed
                // from?" without re-hashing.
                let sha_file = p.manifest_dir.join(".tcr-sha256");
                let sha = std::fs::read_to_string(&sha_file)
                    .ok()
                    .map(|s| s.trim().to_string());
                (p.manifest.name, sha)
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(error = ?e, "plugin_sync: discover failed; treating as empty");
            Vec::new()
        }
    };

    // 2. Compute diff.
    let diff = compute_diff(&installed, &manifest);

    // 3. Uninstall what the coordinator doesn't want.
    for name in &diff.to_remove {
        match uninstaller::uninstall_by_name(plugins_dir, name) {
            Ok(_) => tracing::info!(name = %name, "plugin_sync: uninstalled"),
            Err(e) => tracing::warn!(name = %name, error = ?e, "plugin_sync: uninstall failed; skipping"),
        }
    }

    // 4. Install what's missing or version-bumped.
    for entry in &diff.to_install {
        let index_entry = IndexEntry {
            name: entry.name.clone(),
            version: entry.version.clone(),
            summary: String::new(),
            tarball_url: entry.tarball_url.clone(),
            tarball_sha256: entry.sha256.clone(),
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: Vec::new(),
            runtimes: Vec::new(),
            deps: None,
        };
        match installer::install_from_entry(
            &index_entry,
            plugins_dir,
            None,
            Some(coordinator_token),
        )
        .await
        {
            Ok(installed) => {
                // Write the sha256 marker so the next sync's
                // `compute_diff` can no-op when the manifest hasn't
                // changed.
                let sha_path = installed.plugin_dir.join(".tcr-sha256");
                let _ = std::fs::write(&sha_path, &installed.tarball_sha256);
                tracing::info!(name = %entry.name, "plugin_sync: installed");
            }
            Err(e) => {
                tracing::warn!(name = %entry.name, error = ?e, "plugin_sync: install failed; skipping");
            }
        }
    }

    // 5. Rebuild the worker's step registry so newly-installed
    //    plugins' steps are resolvable. If discover fails here we
    //    can't rebuild — log and move on. Next sync retries.
    let discovered = match crate::plugins::discover(plugins_dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = ?e, "plugin_sync: post-sync discover failed; registry not rebuilt");
            return;
        }
    };

    // 5a. Run each discovered plugin's `deps` shell command (e.g.
    //     `python3 -m venv ./venv && ./venv/bin/pip install ...`).
    //     Mirrors the coordinator's boot-path behavior in `main.rs`:
    //     idempotent re-runs are fine (pip install of an already-
    //     satisfied package is a no-op), and skipping this leaves
    //     plugins like whisper unable to start because their venv
    //     doesn't exist on the worker host.
    //
    //     Failure logs warn and the plugin still registers — the
    //     subsequent dispatch will fail with a clearer error from
    //     bin/run than the bare "produced no result" we'd see
    //     otherwise.
    for d in &discovered {
        if let Some(deps) = &d.manifest.deps {
            tracing::info!(plugin = %d.manifest.name, "running plugin deps");
            if let Err(e) = crate::plugins::deps::run(&d.manifest_dir, deps, |_, _| {}).await {
                tracing::warn!(
                    plugin = %d.manifest.name,
                    error = %e,
                    "plugin deps failed; dispatched steps will likely fail"
                );
            }
        }
    }

    crate::steps::registry::rebuild_from_discovered(discovered).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi(name: &str, sha: &str) -> PluginInstall {
        PluginInstall {
            name: name.into(),
            version: "1.0".into(),
            sha256: sha.into(),
            tarball_url: format!("http://x/{name}/tarball"),
        }
    }

    #[test]
    fn diff_empty_to_empty_is_empty() {
        let d = compute_diff(&[], &[]);
        assert_eq!(d, Diff { to_install: vec![], to_remove: vec![] });
    }

    #[test]
    fn diff_empty_installed_with_one_in_manifest_installs() {
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&[], &m);
        assert_eq!(d.to_install, m);
        assert!(d.to_remove.is_empty());
    }

    #[test]
    fn diff_matching_sha_is_noop() {
        let installed = vec![("a".into(), Some("aaa".into()))];
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&installed, &m);
        assert!(d.to_install.is_empty());
        assert!(d.to_remove.is_empty());
    }

    #[test]
    fn diff_version_bump_replaces() {
        let installed = vec![("a".into(), Some("aaa".into()))];
        let m = vec![pi("a", "bbb")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("a", "bbb")]);
        assert!(d.to_remove.is_empty(), "version bump uses install path's atomic swap, not remove+install");
    }

    #[test]
    fn diff_replaces_unknown_with_known() {
        let installed = vec![("x".into(), Some("xxx".into()))];
        let m = vec![pi("y", "yyy")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("y", "yyy")]);
        assert_eq!(d.to_remove, vec!["x".to_string()]);
    }

    #[test]
    fn diff_unknown_local_sha_is_treated_as_replace() {
        // Installed plugin has no .tcr-sha256 marker (side-loaded).
        // If manifest mentions it, we should reinstall to bring it
        // under management.
        let installed = vec![("a".into(), None)];
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("a", "aaa")]);
        assert!(d.to_remove.is_empty());
    }
}
