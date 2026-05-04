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
//!   on-disk swap (the caller `rm -rf`s the plugin dir). The handler
//!   passes a callback that fans each output line out to an SSE
//!   stream so the UI shows pip's progress in real time.
//! - **Boot path** runs deps in `main.rs` before the step registry is
//!   built. A non-zero exit logs `tracing::warn!` and the plugin still
//!   registers; flow runs that dispatch the plugin's steps will fail
//!   with a clearer error from `bin/run` itself than they would
//!   without the deps.
//!
//! No sandboxing -- the plugin already runs arbitrary code as the
//! transcoderr user, so a `deps` shell is the same trust boundary.

use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum DepsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("deps command exited {status}: {stderr}")]
    NonZero { status: String, stderr: String },
}

/// Which pipe a captured line came from. Useful for the SSE stream UI to
/// label lines, but the run() function itself doesn't care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Run `deps` via `/bin/sh -c` in `plugin_dir`. Streams stdout + stderr
/// line-by-line to `on_line` as they appear. On non-zero exit returns
/// the accumulated stderr (or stdout fallback) in the error.
///
/// The callback runs synchronously per line, so it should be cheap --
/// e.g. forward to an `mpsc::UnboundedSender`. Pass `|_, _| {}` if you
/// don't need streaming.
pub async fn run<F>(plugin_dir: &Path, deps: &str, mut on_line: F) -> Result<(), DepsError>
where
    F: FnMut(Stream, &str),
{
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(deps)
        .current_dir(plugin_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut stdout_lines = BufReader::new(stdout).lines();
    let mut stderr_lines = BufReader::new(stderr).lines();

    // Accumulate stderr (with stdout fallback) so a non-zero exit
    // still produces a useful error message even when the caller's
    // on_line callback is a no-op.
    let mut stderr_buf = String::new();
    let mut stdout_buf = String::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;

    while !(stdout_eof && stderr_eof) {
        tokio::select! {
            line = stdout_lines.next_line(), if !stdout_eof => match line? {
                Some(l) => {
                    stdout_buf.push_str(&l);
                    stdout_buf.push('\n');
                    on_line(Stream::Stdout, &l);
                }
                None => stdout_eof = true,
            },
            line = stderr_lines.next_line(), if !stderr_eof => match line? {
                Some(l) => {
                    stderr_buf.push_str(&l);
                    stderr_buf.push('\n');
                    on_line(Stream::Stderr, &l);
                }
                None => stderr_eof = true,
            },
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let trimmed_err = stderr_buf.trim().to_string();
        return Err(DepsError::NonZero {
            status: status.to_string(),
            stderr: if trimmed_err.is_empty() {
                stdout_buf.trim().to_string()
            } else {
                trimmed_err
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
        run(dir.path(), "true", |_, _| {}).await.unwrap();
    }

    #[tokio::test]
    async fn non_zero_exit_returns_err_with_stderr() {
        let dir = tempdir().unwrap();
        let err = run(dir.path(), "echo nope >&2 && false", |_, _| {})
            .await
            .unwrap_err();
        match err {
            DepsError::NonZero { stderr, .. } => assert_eq!(stderr, "nope"),
            other => panic!("expected NonZero, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn relative_paths_resolve_inside_plugin_dir() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("requirements.txt"), "anyio\n").unwrap();
        run(dir.path(), "test -f requirements.txt", |_, _| {})
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn streams_lines_to_callback_in_order() {
        let dir = tempdir().unwrap();
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(Stream, String)>::new()));
        let lines_clone = lines.clone();
        run(dir.path(), "echo a; echo b >&2; echo c", move |s, line| {
            lines_clone.lock().unwrap().push((s, line.to_string()));
        })
        .await
        .unwrap();
        let captured = lines.lock().unwrap().clone();
        // Order between stdout/stderr isn't strictly defined (the
        // shell schedules them independently), but we should see all
        // three lines and "b" must be tagged Stderr.
        assert_eq!(captured.len(), 3);
        let stderr_lines: Vec<_> = captured
            .iter()
            .filter(|(s, _)| *s == Stream::Stderr)
            .map(|(_, l)| l.clone())
            .collect();
        assert_eq!(stderr_lines, vec!["b"]);
    }
}
