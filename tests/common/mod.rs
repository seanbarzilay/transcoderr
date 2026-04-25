use std::path::PathBuf;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use transcoderr::{
    config::{Config, RadarrConfig},
    db,
    hw::{semaphores::DeviceRegistry, HwCaps},
    http,
    ready::Readiness,
    worker::Worker,
};

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
    transcoderr::steps::registry::init(
        pool.clone(),
        hw_devices.clone(),
        vec![],
    )
    .await;

    let cfg = std::sync::Arc::new(Config {
        bind: "127.0.0.1:0".into(),
        data_dir: data_dir.clone(),
        radarr: RadarrConfig { bearer_token: "test-token".into() },
    });

    let bus = transcoderr::bus::Bus::default();
    let worker = Worker::new(pool.clone(), bus.clone());
    let (tx, rx) = tokio::sync::watch::channel(false);
    let w = tokio::spawn(async move { worker.run_loop(rx).await });

    let ready = Readiness::new();
    ready.mark_ready().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = http::router(http::AppState {
        pool: pool.clone(),
        cfg,
        hw_caps,
        hw_devices,
        bus,
        ready,
    });
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
