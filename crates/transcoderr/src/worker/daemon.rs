//! Worker daemon entry point. Probes hardware, discovers installed
//! plugins, then hands off to `connection::run` which is the long-lived
//! reconnect loop.
//!
//! Boot order:
//!   1. Try to load `worker.toml`.
//!   2. If missing → run mDNS discovery + enrollment, write the file.
//!   3. Probe the WS upgrade once. If 401, wipe + re-enroll exactly
//!      once. If still 401, exit. Other errors fall through to the
//!      long-lived reconnect loop, which has its own backoff.

use crate::worker::config::WorkerConfig;
use crate::worker::connection::{probe_token, ProbeOutcome};
use std::path::{Path, PathBuf};

/// Run the worker daemon. Blocks forever (or exits the process via
/// `std::process::exit` on unrecoverable errors).
pub async fn run(cfg_path: PathBuf) -> ! {
    let cfg = match boot_config(&cfg_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "worker boot failed");
            std::process::exit(1);
        }
    };

    let name = cfg.resolved_name();
    tracing::info!(
        name = %name,
        coordinator = %cfg.coordinator_url,
        "starting worker daemon"
    );

    let caps = crate::ffmpeg_caps::FfmpegCaps::probe().await;
    // Full hardware probe (encoders + GPU enumeration) for the
    // coordinator's Workers UI. Send the rich shape over the wire so
    // the row's hardware column shows the actual GPU lineup instead
    // of falling back to "software only". The simpler `caps` above is
    // still consumed by the local step registry; both probes are
    // boot-time only.
    let hw_caps_full = crate::hw::probe::probe().await;
    let hw_caps = serde_json::to_value(&hw_caps_full)
        .unwrap_or_else(|_| serde_json::json!({}));

    let pool = match crate::db::open_in_memory().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = ?e, "worker: failed to open in-memory sqlite for registry; aborting");
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }
    };

    crate::steps::registry::init(
        pool.clone(),
        crate::hw::semaphores::DeviceRegistry::from_caps(&crate::hw::HwCaps::default()),
        std::sync::Arc::new(caps.clone()),
        Vec::new(),
    )
    .await;

    let ctx = crate::worker::connection::ConnectionContext {
        plugins_dir: std::path::PathBuf::from("./plugins"),
        coordinator_token: cfg.coordinator_token.clone(),
        name: name.clone(),
        hw_caps: hw_caps.clone(),
    };

    crate::worker::connection::run(
        cfg.coordinator_url,
        cfg.coordinator_token,
        ctx,
    )
    .await
}

/// Resolve a usable `WorkerConfig`, performing auto-discovery and 401
/// recovery if needed. Returns `Err` only on terminal failure.
async fn boot_config(cfg_path: &Path) -> anyhow::Result<WorkerConfig> {
    let initial = match WorkerConfig::load(cfg_path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::info!(
                path = %cfg_path.display(),
                error = %e,
                "no usable worker.toml; running auto-discovery"
            );
            None
        }
    };

    let cfg = match initial {
        Some(c) => c,
        None => {
            crate::worker::enroll::discover_and_enroll(cfg_path, None, false).await?
        }
    };

    // Probe once to detect a stale cached token.
    match probe_token(&cfg.coordinator_url, &cfg.coordinator_token).await {
        ProbeOutcome::Ok => Ok(cfg),
        ProbeOutcome::Unauthorized => {
            tracing::warn!(
                "cached coordinator token rejected; deleting {} and re-running discovery",
                cfg_path.display()
            );
            let _ = std::fs::remove_file(cfg_path);
            let new_cfg = crate::worker::enroll::discover_and_enroll(cfg_path, None, false).await?;
            // Second probe — if STILL 401, give up.
            match probe_token(&new_cfg.coordinator_url, &new_cfg.coordinator_token).await {
                ProbeOutcome::Ok => Ok(new_cfg),
                ProbeOutcome::Unauthorized => Err(anyhow::anyhow!(
                    "freshly enrolled token was rejected with 401; refusing to loop"
                )),
                ProbeOutcome::Other(e) => {
                    // Transient — let the reconnect loop deal with it.
                    tracing::warn!(error = %e, "second probe failed; entering reconnect loop");
                    Ok(new_cfg)
                }
            }
        }
        ProbeOutcome::Other(e) => {
            tracing::warn!(
                error = %e,
                "initial probe failed; entering reconnect loop with current cfg"
            );
            Ok(cfg)
        }
    }
}
