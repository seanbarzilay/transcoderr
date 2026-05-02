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

/// Drive a plugin install endpoint to its terminal SSE event. Returns
/// `Ok(installed_name)` if the stream emits `event: done` and `Err((status, message))`
/// if it emits `event: error`. Used by integration tests in place of the
/// pre-SSE pattern of asserting `resp.status() == 422` on the POST itself --
/// the install endpoint always returns 200 now; the actual outcome is in
/// the event stream.
#[allow(dead_code)]
pub async fn install_via_sse(
    client: &reqwest::Client,
    url: &str,
) -> Result<String, (u16, String)> {
    use futures::StreamExt;
    let resp = client.post(url).send().await.unwrap();
    assert_eq!(resp.status(), 200, "install endpoint must return 200 SSE; got {}", resp.status());
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.unwrap();
        buf.push_str(std::str::from_utf8(&bytes).unwrap());
        // Each SSE frame is "event: <name>\ndata: <json>\n\n"; parse them as
        // they arrive so we can return on the first terminal frame.
        while let Some(end) = buf.find("\n\n") {
            let frame = buf[..end].to_string();
            buf.drain(..end + 2);
            let mut event = "message".to_string();
            let mut data_lines: Vec<&str> = Vec::new();
            for line in frame.lines() {
                if let Some(v) = line.strip_prefix("event:") { event = v.trim().to_string(); }
                else if let Some(v) = line.strip_prefix("data:") { data_lines.push(v.trim()); }
            }
            if data_lines.is_empty() { continue; }
            let data: serde_json::Value = serde_json::from_str(&data_lines.join("\n"))
                .unwrap_or(serde_json::Value::Null);
            match event.as_str() {
                "done" => {
                    return Ok(data["installed"].as_str().unwrap_or("").to_string());
                }
                "error" => {
                    let status = data["status"].as_u64().unwrap_or(500) as u16;
                    let msg = data["message"].as_str().unwrap_or("").to_string();
                    return Err((status, msg));
                }
                _ => continue,
            }
        }
    }
    panic!("install SSE stream ended without a terminal `done` or `error` event");
}
