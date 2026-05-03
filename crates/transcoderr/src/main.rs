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
    /// Run the server (coordinator).
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Run as a remote worker. Connects to a coordinator over WebSocket.
    Worker {
        /// Path to worker.toml. Default is the Docker-friendly
        /// /var/lib/transcoderr/worker.toml; override with `--config`
        /// for non-container or non-default deployments. If the file
        /// is missing on first boot, the worker auto-discovers a
        /// coordinator via mDNS and writes the file at this path.
        #[arg(long, default_value = "/var/lib/transcoderr/worker.toml")]
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

            // Diagnostic warning: a manually-installed plugin whose
            // declared runtimes aren't on $PATH will register fine but
            // fail at first dispatch. Surfacing this at boot makes the
            // failure mode debuggable from the server log.
            let runtime_checker = std::sync::Arc::new(
                transcoderr::plugins::runtime::RuntimeChecker::default(),
            );
            for d in &discovered {
                let missing = runtime_checker.missing(&d.manifest.runtimes).await;
                if !missing.is_empty() {
                    tracing::warn!(
                        plugin = %d.manifest.name,
                        missing = ?missing,
                        "plugin declares runtime(s) not on PATH; runs that dispatch its steps will fail until they're installed",
                    );
                }
            }

            // Run each plugin's `deps` shell command (e.g.
            // `pip install -r requirements.txt`) before the registry
            // is built, so the plugin's runtime imports are satisfied
            // when its steps eventually dispatch. Failure logs warn
            // and the plugin still registers; the operator sees the
            // error in the server log AND the plugin will eventually
            // fail at dispatch with a clearer error from bin/run.
            for d in &discovered {
                if let Some(deps) = &d.manifest.deps {
                    tracing::info!(plugin = %d.manifest.name, "running plugin deps");
                    if let Err(e) = transcoderr::plugins::deps::run(&d.manifest_dir, deps, |_, _| {}).await {
                        tracing::warn!(
                            plugin = %d.manifest.name,
                            error = %e,
                            "plugin deps failed at boot; flow runs that dispatch its steps will likely fail"
                        );
                    }
                }
            }
            let ffmpeg_caps = std::sync::Arc::new(
                transcoderr::ffmpeg_caps::FfmpegCaps::probe().await,
            );
            tracing::info!(
                libplacebo = ffmpeg_caps.has_libplacebo,
                "ffmpeg caps probed",
            );

            transcoderr::worker::local::register_local_worker(
                &pool,
                &ffmpeg_caps,
                &discovered,
            )
            .await?;

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
            transcoderr::worker::local::spawn_local_heartbeat(pool.clone());

            // Bind the HTTP listener early so we can resolve the
            // public URL and assemble `AppState` *before* spawning the
            // worker pool. The pool's worker uses the dispatcher in
            // `AppState` (Piece 3) to route remote-eligible steps, so
            // it has to be passed an already-built `AppState`.
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
            // mDNS auto-discovery responder. Binds to the actual port
            // (covers `bind = "0.0.0.0:0"` ephemeral case). Disable via
            // `TRANSCODERR_DISCOVERY=disabled`. Held for the process
            // lifetime; drops on shutdown.
            let _mdns = if std::env::var("TRANSCODERR_DISCOVERY").as_deref() == Ok("disabled") {
                tracing::info!("TRANSCODERR_DISCOVERY=disabled; mDNS responder skipped");
                None
            } else {
                let instance = hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| format!("transcoderr-{}", uuid::Uuid::new_v4()));
                match transcoderr::discovery::start_responder(addr.port(), &instance) {
                    Ok(d) => Some(d),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "failed to start mDNS responder; coordinator will run without LAN auto-discovery"
                        );
                        None
                    }
                }
            };
            let public_url_arc = std::sync::Arc::new(public_url.url);

            let arr_cache = std::sync::Arc::new(transcoderr::arr::cache::ArrCache::new(
                std::time::Duration::from_secs(300),
            ));

            let ready = transcoderr::ready::Readiness::new();

            let state = transcoderr::http::AppState {
                pool: pool.clone(),
                cfg: cfg.clone(),
                hw_caps,
                hw_devices: registry,
                ffmpeg_caps: ffmpeg_caps.clone(),
                bus: bus.clone(),
                ready: ready.clone(),
                metrics,
                cancellations: cancellations.clone(),
                public_url: public_url_arc,
                arr_cache,
                catalog_client: std::sync::Arc::new(transcoderr::plugins::catalog::CatalogClient::default()),
                runtime_checker: runtime_checker.clone(),
                connections: transcoderr::worker::connections::Connections::new(),
            };

            let worker = transcoderr::worker::Worker::with_state(state.clone());
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

            ready.mark_ready().await;

            transcoderr::arr::reconcile::spawn(state.pool.clone(), state.public_url.clone());

            transcoderr::api::workers::spawn_idle_sweep(state.clone()).await;

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
        Cmd::Worker { config } => {
            transcoderr::worker::daemon::run(config).await
        }
    }
}
