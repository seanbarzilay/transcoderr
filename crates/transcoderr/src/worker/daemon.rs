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

    crate::worker::connection::run(
        config.coordinator_url,
        config.coordinator_token,
        build_register,
    )
    .await
}
