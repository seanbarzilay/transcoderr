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

            let (tx, rx) = tokio::sync::watch::channel(false);
            let worker_task =
                tokio::spawn(async move { worker.run_loop(rx).await });

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
                bus,
                ready: ready.clone(),
                metrics,
                cancellations,
                public_url: public_url_arc,
                arr_cache,
            };
            ready.mark_ready().await;

            transcoderr::arr::reconcile::spawn(state.pool.clone(), state.public_url.clone());

            let app = transcoderr::http::router(state);

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
