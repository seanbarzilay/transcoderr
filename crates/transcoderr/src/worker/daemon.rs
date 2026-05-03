//! Worker daemon entry point. Probes hardware, discovers installed
//! plugins, then hands off to `connection::run` which is the long-lived
//! reconnect loop.

use crate::worker::config::WorkerConfig;
use crate::worker::protocol::{Envelope, Message, PluginManifestEntry, Register};
use std::path::Path;

pub async fn run(config: WorkerConfig) -> ! {
    let name = config.resolved_name();
    tracing::info!(name = %name, coordinator = %config.coordinator_url, "starting worker daemon");

    let caps = crate::ffmpeg_caps::FfmpegCaps::probe().await;
    let hw_caps = serde_json::json!({
        "has_libplacebo": caps.has_libplacebo,
    });

    let plugin_manifest: Vec<PluginManifestEntry> = match crate::plugins::discover(Path::new("./plugins")) {
        Ok(found) => found
            .into_iter()
            .map(|d| PluginManifestEntry {
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                sha256: None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = ?e, "plugin discovery failed; reporting empty manifest");
            Vec::new()
        }
    };

    // Piece 3: initialise the step registry on the worker side so
    // executor::handle_step_dispatch can resolve step kinds. Open a
    // process-local in-memory sqlite for any built-in that consults
    // settings or scratch tables.
    let pool = match crate::db::open_in_memory().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = ?e, "worker: failed to open in-memory sqlite for registry; aborting");
            // Sleep forever to make systemd retry visibly.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }
    };

    crate::steps::registry::init(
        pool.clone(),
        crate::hw::semaphores::DeviceRegistry::from_caps(&crate::hw::HwCaps::default()),
        std::sync::Arc::new(caps.clone()),
        Vec::new(), // no plugins on the worker side until Piece 4 ships push
    )
    .await;

    let available_steps = vec![
        "plan.execute".into(),
        "transcode".into(),
        "remux".into(),
        "extract.subs".into(),
        "iso.extract".into(),
        "audio.ensure".into(),
        "strip.tracks".into(),
    ];

    let build_register = move || -> Envelope {
        Envelope {
            id: format!("reg-{}", uuid::Uuid::new_v4()),
            message: Message::Register(Register {
                name: name.clone(),
                version: env!("CARGO_PKG_VERSION").into(),
                hw_caps: hw_caps.clone(),
                available_steps: available_steps.clone(),
                plugin_manifest: plugin_manifest.clone(),
            }),
        }
    };

    let ctx = crate::worker::connection::ConnectionContext {
        plugins_dir: std::path::PathBuf::from("./plugins"),
        coordinator_token: config.coordinator_token.clone(),
    };

    crate::worker::connection::run(
        config.coordinator_url,
        config.coordinator_token,
        build_register,
        ctx,
    )
    .await
}
