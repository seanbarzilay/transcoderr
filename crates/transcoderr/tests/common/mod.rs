use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use transcoderr::{
    config::{Config, RadarrConfig},
    db,
    hw::{semaphores::DeviceRegistry, HwCaps},
    http,
    metrics::Metrics,
    ready::Readiness,
    worker::Worker,
};

static METRICS: OnceLock<Arc<Metrics>> = OnceLock::new();

// Each integration test binary uses a different subset of these fields,
// so per-binary dead_code warnings are unavoidable. Allow them at the
// struct level rather than playing whack-a-mole.
#[allow(dead_code)]
pub struct TestApp {
    pub url: String,
    pub pool: sqlx::SqlitePool,
    pub data_dir: PathBuf,
    _temp: TempDir,
    _server: JoinHandle<()>,
    _worker: JoinHandle<()>,
    // Keep the sender alive so the worker's shutdown watch doesn't fire immediately.
    _shutdown_tx: tokio::sync::watch::Sender<bool>,
}

pub async fn boot() -> TestApp {
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().to_path_buf();
    let pool = db::open(&data_dir).await.unwrap();

    // Empty hw caps for tests.
    let caps = HwCaps::default();
    let hw_devices = DeviceRegistry::from_caps(&caps);
    let hw_caps = std::sync::Arc::new(tokio::sync::RwLock::new(caps));

    // Initialize the step registry with no subprocess plugins for tests.
    let ffmpeg_caps = std::sync::Arc::new(transcoderr::ffmpeg_caps::FfmpegCaps::default());
    transcoderr::steps::registry::init(
        pool.clone(),
        hw_devices.clone(),
        ffmpeg_caps.clone(),
        vec![],
    )
    .await;

    let cfg = std::sync::Arc::new(Config {
        bind: "127.0.0.1:0".into(),
        data_dir: data_dir.clone(),
        radarr: RadarrConfig { bearer_token: "test-token".into() },
    });

    let bus = transcoderr::bus::Bus::default();
    let cancellations = transcoderr::cancellation::JobCancellations::new();
    let worker = Worker::new(pool.clone(), bus.clone(), data_dir.clone(), cancellations.clone());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let w = tokio::spawn(async move { worker.run_loop(rx).await });

    let ready = Readiness::new();
    ready.mark_ready().await;

    let metrics = METRICS.get_or_init(|| Arc::new(Metrics::install().unwrap())).clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http::router(
        http::AppState {
            pool: pool.clone(),
            cfg,
            hw_caps,
            hw_devices,
            ffmpeg_caps,
            bus,
            ready,
            metrics,
            cancellations,
            public_url: std::sync::Arc::new("http://test:8099".to_string()),
            arr_cache: std::sync::Arc::new(transcoderr::arr::cache::ArrCache::new(
                std::time::Duration::from_secs(300),
            )),
            catalog_client: std::sync::Arc::new(transcoderr::plugins::catalog::CatalogClient::default()),
            runtime_checker: std::sync::Arc::new(transcoderr::plugins::runtime::RuntimeChecker::default()),
        },
        std::time::Duration::from_secs(300),
    );
    let s = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        url: format!("http://{addr}"),
        pool,
        data_dir,
        _temp: temp,
        _server: s,
        _worker: w,
        _shutdown_tx: tx,
    }
}
