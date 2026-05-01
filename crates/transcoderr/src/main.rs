#[allow(unused_imports)]
use transcoderr::{config, db, error};

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "transcoderr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Run the server.
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "transcoderr=info,tower_http=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { config } => {
            let cfg = std::sync::Arc::new(
                transcoderr::config::Config::from_path(&config)?,
            );
            let pool = transcoderr::db::open(&cfg.data_dir).await?;

            // Probe hardware capabilities at boot.
            let caps = transcoderr::hw::probe::probe().await;
            let _ = transcoderr::db::snapshot_hw_caps(&pool, &caps).await;
            let registry = transcoderr::hw::semaphores::DeviceRegistry::from_caps(&caps);
            let hw_caps = std::sync::Arc::new(tokio::sync::RwLock::new(caps));

            // Discover plugins and initialize the step registry.
            let plugins_dir = cfg.data_dir.join("plugins");
            let discovered = transcoderr::plugins::discover(&plugins_dir)?;
            // Mirror the on-disk set into the `plugins` DB table so the UI
            // page lists what was discovered. Without this the UI is
            // permanently empty even though the in-memory step registry
            // happily dispatches the steps.
            transcoderr::db::plugins::sync_discovered(&pool, &discovered, &std::collections::HashMap::new()).await?;
            let ffmpeg_caps = std::sync::Arc::new(
                transcoderr::ffmpeg_caps::FfmpegCaps::probe().await,
            );
            tracing::info!(
                libplacebo = ffmpeg_caps.has_libplacebo,
                "ffmpeg caps probed",
            );

            transcoderr::steps::registry::init(
                pool.clone(),
                registry.clone(),
                ffmpeg_caps.clone(),
                discovered,
            )
            .await;

            let metrics = std::sync::Arc::new(transcoderr::metrics::Metrics::install()?);

            let bus = transcoderr::bus::Bus::default();
            let cancellations = transcoderr::cancellation::JobCancellations::new();
            let worker = transcoderr::worker::Worker::new(
                pool.clone(),
                bus.clone(),
                cfg.data_dir.clone(),
                cancellations.clone(),
            );
            let reset = worker.recover_on_boot().await?;
            if reset > 0 {
                tracing::warn!(reset, "recovered stale running jobs");
            }

            // Concurrency: spawn N worker loops sharing the same pool.
            // claim_next is atomic (UPDATE...WHERE status='pending'), so
            // they cooperate without stepping on each other. Hardware
            // semaphores in DeviceRegistry still cap concurrent ffmpeg
            // invocations on each GPU, so this only affects how many
            // jobs run in parallel — not how many ffmpegs hit one GPU.
            let pool_size: usize = transcoderr::db::settings::get(&pool, "worker.pool_size")
                .await
                .ok()
                .flatten()
                .and_then(|s| s.parse().ok())
                .filter(|n: &usize| *n >= 1)
                .unwrap_or(1);
            tracing::info!(pool_size, "spawning worker pool");

            let (tx, _) = tokio::sync::watch::channel(false);
            let worker_tasks: Vec<_> = (0..pool_size)
                .map(|_| {
                    let w = worker.clone();
                    let rx = tx.subscribe();
                    tokio::spawn(async move { w.run_loop(rx).await })
                })
                .collect();
            // Keep the original single-handle name for downstream join
            // logic that expects one task. We fold the others into it via
            // join_all at shutdown.
            let worker_task = tokio::spawn(async move {
                for t in worker_tasks {
                    let _ = t.await;
                }
            });

            let retention_rx = tx.subscribe();
            tokio::spawn(transcoderr::retention::run_periodic(pool.clone(), retention_rx));

            let ready = transcoderr::ready::Readiness::new();

            let listener =
                tokio::net::TcpListener::bind(&cfg.bind).await?;
            let addr = listener.local_addr()?;
            let public_url = transcoderr::public_url::resolve(addr);
            tracing::info!(
                public_url = %public_url.url,
                source = ?public_url.source,
                addr = %addr,
                "transcoderr serving",
            );
            let public_url_arc = std::sync::Arc::new(public_url.url);

            let arr_cache = std::sync::Arc::new(transcoderr::arr::cache::ArrCache::new(
                std::time::Duration::from_secs(300),
            ));

            let state = transcoderr::http::AppState {
                pool,
                cfg: cfg.clone(),
                hw_caps,
                hw_devices: registry,
                ffmpeg_caps: ffmpeg_caps.clone(),
                bus,
                ready: ready.clone(),
                metrics,
                cancellations,
                public_url: public_url_arc,
                arr_cache,
                catalog_client: std::sync::Arc::new(transcoderr::plugins::catalog::CatalogClient::default()),
            };
            ready.mark_ready().await;

            transcoderr::arr::reconcile::spawn(state.pool.clone(), state.public_url.clone());

            let dedup_window_secs: u64 = transcoderr::db::settings::get(&state.pool, "dedup.window_seconds")
                .await
                .ok()
                .flatten()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300);
            let app = transcoderr::http::router(state, std::time::Duration::from_secs(dedup_window_secs));

            let serve = axum::serve(listener, app).with_graceful_shutdown(
                async move {
                    let _ = tokio::signal::ctrl_c().await;
                },
            );
            let serve_task = tokio::spawn(async move { serve.await });
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("ctrl-c, shutting down");
            let _ = tx.send(true);
            let _ = serve_task.await;
            let _ = worker_task.await;
            Ok(())
        }
    }
}
