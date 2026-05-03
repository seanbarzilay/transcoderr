//! End-to-end: coordinator advertises via mDNS → worker browses, finds
//! it, POSTs `/api/worker/enroll`, writes `worker.toml`, reads it back.
//!
//! Uses a unique mDNS instance suffix so concurrent test runs don't
//! see each other (or any real coordinator on the dev machine's LAN).

mod common;

use common::boot;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn discover_enroll_and_persist_worker_toml() {
    let app = boot().await;

    // Parse the test app's URL into host:port and start a discovery
    // responder against the same port. The unique suffix ensures we
    // don't collide with concurrent test runs (cargo nextest, parallel
    // `cargo test`, or a real transcoderr running on localhost).
    let port: u16 = app
        .url
        .strip_prefix("http://")
        .and_then(|s| s.rsplit(':').next())
        .and_then(|p| p.parse().ok())
        .expect("test url must contain a port");

    let suffix = format!("auto-discovery-{}", Uuid::new_v4());
    // Pin the responder to 127.0.0.1 so the worker's mDNS resolve
    // produces a loopback address that matches the test HTTP server.
    let _mdns = transcoderr::discovery::start_responder_on_loopback(port, &suffix)
        .expect("start responder");

    // Run the worker-side enrollment routine. Use the unique suffix
    // as an instance filter so we only ever resolve OUR responder.
    // with_loopback=true because both sides run on 127.0.0.1 in tests.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("worker.toml");
    let cfg = transcoderr::worker::enroll::discover_and_enroll(
        &cfg_path,
        Some(suffix.clone()),
        true,
    )
    .await
    .expect("discover + enroll within 5s");

    // The cfg is the freshly-loaded WorkerConfig. The token is non-empty
    // and the URL is ws://...
    assert!(!cfg.coordinator_token.is_empty(), "token must be non-empty");
    assert!(
        cfg.coordinator_url.starts_with("ws://"),
        "coordinator_url must use ws:// scheme: {}",
        cfg.coordinator_url
    );
    assert!(
        cfg.coordinator_url.ends_with("/api/worker/connect"),
        "coordinator_url must end with /api/worker/connect: {}",
        cfg.coordinator_url
    );

    // The file exists and round-trips via WorkerConfig::load.
    assert!(cfg_path.exists(), "worker.toml must exist after enroll");
    let reloaded = transcoderr::worker::config::WorkerConfig::load(&cfg_path).unwrap();
    assert_eq!(reloaded.coordinator_token, cfg.coordinator_token);

    // One row landed in the workers table with kind='remote' and the
    // same token.
    let rows = transcoderr::db::workers::list_all(&app.pool).await.unwrap();
    let mine = rows
        .iter()
        .find(|r| r.kind == "remote" && r.secret_token.as_deref() == Some(cfg.coordinator_token.as_str()))
        .expect("our enrolled row must exist");
    assert!(mine.id > 0);
}

#[tokio::test]
async fn discover_times_out_when_no_responder_present() {
    // Make the deadline very short here by NOT starting any responder
    // and using an instance filter that no one else can match. The
    // 5-second deadline is hard-coded inside discover_and_enroll, so
    // this test takes ~5s wall-clock; that's fine for a single test.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("worker.toml");
    let unique = format!("nope-{}", Uuid::new_v4());
    let res = tokio::time::timeout(
        Duration::from_secs(7),
        transcoderr::worker::enroll::discover_and_enroll(&cfg_path, Some(unique), false),
    )
    .await
    .expect("must complete within 7s wall-clock");
    assert!(res.is_err(), "expected an error when no responder is present");
    assert!(!cfg_path.exists(), "no partial file may be written");
}
