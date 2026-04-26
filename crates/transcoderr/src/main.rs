#[allow(unused_imports)]
use transcoderr::{config, db, error};

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "transcoderr", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    /// Log output format.
    #[arg(long, env = "LOG_FORMAT", value_enum, default_value_t = transcoderr_api_types::logging::LogFormat::Text, global = true)]
    log_format: transcoderr_api_types::logging::LogFormat,
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
    let cli = Cli::parse();
    transcoderr_api_types::logging::init(cli.log_format, "transcoderr=info,tower_http=info");
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

            let state = transcoderr::http::AppState {
                pool,
                cfg: cfg.clone(),
                hw_caps,
                hw_devices: registry,
                bus,
                ready: ready.clone(),
                metrics,
                cancellations,
            };
            ready.mark_ready().await;

            let app = transcoderr::http::router(state);
            let listener =
                tokio::net::TcpListener::bind(&cfg.bind).await?;
            let addr = listener.local_addr()?;
            tracing::info!(addr = %addr, "serving");

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
