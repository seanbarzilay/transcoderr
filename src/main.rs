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
            transcoderr::steps::registry::init(
                pool.clone(),
                registry.clone(),
                discovered,
            )
            .await;

            let metrics = std::sync::Arc::new(transcoderr::metrics::Metrics::install()?);

            let bus = transcoderr::bus::Bus::default();
            let worker = transcoderr::worker::Worker::new(pool.clone(), bus.clone());
            let reset = worker.recover_on_boot().await?;
            if reset > 0 {
                tracing::warn!(reset, "recovered stale running jobs");
            }

            let (tx, rx) = tokio::sync::watch::channel(false);
            let worker_task =
                tokio::spawn(async move { worker.run_loop(rx).await });

            let ready = transcoderr::ready::Readiness::new();

            let state = transcoderr::http::AppState {
                pool,
                cfg: cfg.clone(),
                hw_caps,
                hw_devices: registry,
                bus,
                ready: ready.clone(),
                metrics,
            };
            ready.mark_ready().await;

            let app = transcoderr::http::router(state);
            let listener =
                tokio::net::TcpListener::bind(&cfg.bind).await?;
            tracing::info!(bind = %cfg.bind, "serving");

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
