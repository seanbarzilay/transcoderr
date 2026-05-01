//! PATH-only plugin runtime availability checks.
//!
//! Plugins declare a `runtimes` list in their `manifest.toml` -- the
//! executables they shell out to (e.g. `python3`, `node`, `bash`). The
//! checker verifies each is on `$PATH` with the executable bit set, and
//! caches results so repeated catalog browses don't re-stat the same
//! directories. Missing runtimes block install (with a 422) and surface
//! in the Browse tab so the operator sees *why* an Install button is
//! greyed out before they click.
//!
//! Boot-time discover doesn't refuse to register a plugin whose runtime
//! is missing -- the operator may be about to install the runtime --
//! but main.rs logs a warning so the failure mode is visible in the
//! server log.

use std::collections::HashMap;
use std::path::Path;
use tokio::sync::RwLock;

#[derive(Default)]
pub struct RuntimeChecker {
    cache: RwLock<HashMap<String, bool>>,
}

impl RuntimeChecker {
    pub async fn is_available(&self, name: &str) -> bool {
        if let Some(&v) = self.cache.read().await.get(name) {
            return v;
        }
        let v = which_path(name);
        self.cache.write().await.insert(name.to_string(), v);
        v
    }

    /// Returns the subset of `requested` that are NOT available. Empty
    /// vec means everything's installable.
    pub async fn missing(&self, requested: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for r in requested {
            if !self.is_available(r).await {
                out.push(r.clone());
            }
        }
        out
    }

    /// Bust the cache. Wired to a future "rescan PATH" admin action --
    /// not exposed yet, but keeps the operator's path forward open.
    #[allow(dead_code)]
    pub async fn invalidate(&self) {
        self.cache.write().await.clear();
    }
}

/// Look up `name` on `$PATH`. Returns true iff at least one PATH entry
/// contains a regular file by that name with the executable bit set
/// (Unix). Always false on Windows since transcoderr is documented as
/// Linux/macOS-only.
fn which_path(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains('\0') {
        // Plugin authors declare bare executable names; reject any
        // separator chars so a typo or hostile manifest can't pivot
        // the check into "does /etc/passwd exist?".
        return false;
    }
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let p = Path::new(dir).join(name);
        if is_executable(&p) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sh_is_always_available() {
        // Every supported transcoderr image (CPU, Intel, NVIDIA, full) plus
        // every dev workstation has /bin/sh -- pin this so a regression in
        // the PATH lookup is loud.
        let checker = RuntimeChecker::default();
        assert!(checker.is_available("sh").await);
    }

    #[tokio::test]
    async fn unknown_runtime_is_not_available() {
        let checker = RuntimeChecker::default();
        assert!(!checker.is_available("definitely-not-a-real-binary-12345abcxyz").await);
    }

    #[tokio::test]
    async fn missing_returns_only_missing_subset() {
        let checker = RuntimeChecker::default();
        let missing = checker
            .missing(&["sh".into(), "definitely-fake-9876xyz".into()])
            .await;
        assert_eq!(missing, vec!["definitely-fake-9876xyz".to_string()]);
    }

    #[tokio::test]
    async fn rejects_path_separators_in_name() {
        let checker = RuntimeChecker::default();
        // `/bin/sh` definitely exists on the host but the checker should
        // refuse to look at full paths -- runtimes are bare executable
        // names by contract.
        assert!(!checker.is_available("/bin/sh").await);
        assert!(!checker.is_available("../etc/passwd").await);
        assert!(!checker.is_available("").await);
    }

    #[tokio::test]
    async fn caches_lookups() {
        let checker = RuntimeChecker::default();
        // Prime the cache with a missing one.
        checker.is_available("definitely-fake-555").await;
        assert_eq!(checker.cache.read().await.get("definitely-fake-555"), Some(&false));

        // Manually flip the cached value (something the prod code never
        // does, but proves we're hitting the cache).
        checker
            .cache
            .write()
            .await
            .insert("definitely-fake-555".into(), true);
        assert!(checker.is_available("definitely-fake-555").await);
    }
}
