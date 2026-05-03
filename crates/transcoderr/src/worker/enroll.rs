//! Worker-side enrollment: discover the coordinator via mDNS,
//! POST `/api/worker/enroll`, write the resulting config to disk.
//!
//! Combines `worker::discovery::browse` and a single HTTP POST into
//! one operation. Called from `daemon::run` (Task 5) when no
//! `worker.toml` exists at boot.

use crate::worker::config::WorkerConfig;
use crate::worker::discovery::{browse, DiscoveredCoordinator};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const BROWSE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
struct EnrollReq<'a> {
    name: &'a str,
}

#[derive(Debug, Deserialize)]
struct EnrollResp {
    #[allow(dead_code)] // returned by server; logged but not used here
    id: i64,
    secret_token: String,
    ws_url: String,
}

/// Discover a coordinator on the LAN, enroll, and write the resulting
/// config to `cfg_path`. Used when no `worker.toml` exists.
///
/// `instance_filter`: when `Some`, restricts mDNS results to instances
/// whose fullname contains the given substring. Used by the integration
/// test to isolate concurrent runs; production callers pass `None`.
pub async fn discover_and_enroll(
    cfg_path: &Path,
    instance_filter: Option<String>,
) -> anyhow::Result<WorkerConfig> {
    let coord = browse(BROWSE_DEADLINE, instance_filter)
        .await?
        .ok_or_else(|| anyhow::anyhow!(
            "no coordinator found on the LAN within {BROWSE_DEADLINE:?} — \
             see docs/deploy.md for manual config"
        ))?;
    tracing::info!(addr = %coord.addr, port = coord.port, "discovered coordinator");

    let name = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unnamed-worker".into());

    let resp = post_enroll(&coord, &name).await?;
    write_config(cfg_path, &resp.ws_url, &resp.secret_token, &name)?;
    tracing::info!(path = %cfg_path.display(), "wrote auto-enrolled worker.toml");

    WorkerConfig::load(cfg_path).context("re-load freshly written worker.toml")
}

async fn post_enroll(
    coord: &DiscoveredCoordinator,
    name: &str,
) -> anyhow::Result<EnrollResp> {
    let url = format!("{}{}", coord.http_url(), coord.enroll_path);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&EnrollReq { name })
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
        anyhow::bail!("enroll {url} returned {status}: {body}");
    }
    resp.json::<EnrollResp>().await.context("parse enroll response")
}

/// Write a `worker.toml` at `path` with the given fields. Creates the
/// parent directory if missing.
pub fn write_config(
    path: &Path,
    coordinator_url: &str,
    coordinator_token: &str,
    name: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    let body = format!(
        "coordinator_url   = \"{coordinator_url}\"\n\
         coordinator_token = \"{coordinator_token}\"\n\
         name              = \"{name}\"\n"
    );
    std::fs::write(path, body)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_config_round_trips_through_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");
        write_config(
            &path,
            "ws://192.168.1.50:8765/api/worker/connect",
            "abcdef0123456789",
            "fluffy-1",
        )
        .unwrap();
        let cfg = WorkerConfig::load(&path).unwrap();
        assert_eq!(cfg.coordinator_url, "ws://192.168.1.50:8765/api/worker/connect");
        assert_eq!(cfg.coordinator_token, "abcdef0123456789");
        assert_eq!(cfg.name.as_deref(), Some("fluffy-1"));
    }

    #[test]
    fn write_config_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("worker.toml");
        write_config(&nested, "ws://x/api", "tok", "n").unwrap();
        assert!(nested.exists());
        // The contents must round-trip too.
        let cfg = WorkerConfig::load(&nested).unwrap();
        assert_eq!(cfg.coordinator_token, "tok");
    }
}
