//! Run a plugin's declared `deps` shell command.
//!
//! Plugins declare an optional `deps` string in their `manifest.toml`
//! (e.g. `deps = "pip install -r requirements.txt"`). The server runs
//! the command via `/bin/sh -c` from inside the plugin's directory --
//! so relative paths in the command (`requirements.txt`,
//! `package.json`, etc.) resolve correctly -- and surfaces stdout +
//! stderr in the failure error message.
//!
//! Two call sites:
//! - **Install handler** runs deps after `install_from_entry`. A
//!   non-zero exit refuses the install with a 422 and rolls back the
//!   on-disk swap (the caller `rm -rf`s the plugin dir).
//! - **Boot path** runs deps in `main.rs` before the step registry is
//!   built. A non-zero exit logs `tracing::warn!` and the plugin still
//!   registers; flow runs that dispatch the plugin's steps will fail
//!   with a clearer error from `bin/run` itself than they would
//!   without the deps.
//!
//! No sandboxing -- the plugin already runs arbitrary code as the
//! transcoderr user, so a `deps` shell is the same trust boundary.

use std::path::Path;
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("deps command exited {status}: {stderr}")]
    NonZero { status: String, stderr: String },
}

/// Run `deps` via `/bin/sh -c` in `plugin_dir`. Captures stdout +
/// stderr; on non-zero exit returns the trimmed stderr in the error.
pub async fn run(plugin_dir: &Path, deps: &str) -> Result<(), DepsError> {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(deps)
        .current_dir(plugin_dir)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DepsError::NonZero {
            status: output.status.to_string(),
            stderr: if stderr.is_empty() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                stderr
            },
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn ok_command_returns_ok() {
        let dir = tempdir().unwrap();
        run(dir.path(), "true").await.unwrap();
    }

    #[tokio::test]
    async fn non_zero_exit_returns_err_with_stderr() {
        let dir = tempdir().unwrap();
        let err = run(dir.path(), "echo nope >&2 && false").await.unwrap_err();
        match err {
            DepsError::NonZero { stderr, .. } => assert_eq!(stderr, "nope"),
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn relative_paths_resolve_inside_plugin_dir() {
        // Drop a sentinel file in the plugin dir; have the deps
        // command consume it. Proves cwd is plugin_dir, not /.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "anyio\n").unwrap();
        run(dir.path(), "test -f requirements.txt").await.unwrap();
    }
}
