# Worker Auto-Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A worker started with no `worker.toml` discovers the coordinator over mDNS on the LAN, enrolls itself for a token, persists the resulting config to disk, and proceeds through the existing connect handshake — all in one command.

**Architecture:** Coordinator advertises `_transcoderr._tcp.local.` via `mdns-sd` (pure-Rust). New unauthenticated `POST /api/worker/enroll` endpoint mints a token using the existing `db::workers::insert_remote` helper. Worker-side: on boot, if `WorkerConfig::load` fails, browse mDNS (5 s deadline), POST `/api/worker/enroll`, write the resulting `worker.toml`, then connect. WS-upgrade `401` with a cached token triggers a single recovery cycle (delete file + re-discover + re-enroll) before exiting.

**Tech Stack:** Rust 2021 (axum 0.7, tokio, sqlx + sqlite, anyhow, tracing). One new dep: `mdns-sd` (pure-Rust, no avahi/Bonjour system requirement). No DB migration. No new `[dev-dependencies]`.

**Branch:** all tasks land on a fresh `feat/worker-auto-discovery` branch off `main`. The implementer creates the branch before Task 1.

---

## File Structure

**New backend files:**
- `crates/transcoderr/src/discovery/mod.rs` — coordinator-side mDNS responder. One public fn `start_responder(port, instance_name) -> anyhow::Result<ServiceDaemon>`.
- `crates/transcoderr/src/api/worker_enroll.rs` — `POST /api/worker/enroll` handler. `EnrollReq { name }`, `EnrollResp { id, secret_token, ws_url }`.
- `crates/transcoderr/src/worker/discovery.rs` — worker-side mDNS browser. One public fn `browse(deadline, instance_filter) -> anyhow::Result<Option<DiscoveredCoordinator>>` and a small `parse_service_info` helper kept private but unit-testable via `#[cfg(test)]`.
- `crates/transcoderr/src/worker/enroll.rs` — combines the discovery + POST + write-file flow. Public fns: `discover_and_enroll(cfg_path) -> anyhow::Result<WorkerConfig>` and `write_config(path, ...) -> anyhow::Result<()>`.
- `crates/transcoderr/tests/auto_discovery.rs` — single end-to-end integration test.
- `crates/transcoderr/tests/worker_enroll.rs` — handler-level integration test for the new endpoint (mirrors `tests/api_auth.rs` style).

**Modified backend files:**
- `crates/transcoderr/Cargo.toml` — add `mdns-sd = "0.13"`.
- `crates/transcoderr/src/lib.rs` — `pub mod discovery;`.
- `crates/transcoderr/src/api/mod.rs` — `pub mod worker_enroll;` + new public route.
- `crates/transcoderr/src/worker/mod.rs` — `pub mod discovery;` + `pub mod enroll;`.
- `crates/transcoderr/src/worker/connection.rs` — new `probe_token(url, token)` helper that does a single WS-upgrade attempt and classifies the outcome (`Ok`, `Unauthorized`, `Other`). Existing `run` and `connect_once` unchanged.
- `crates/transcoderr/src/worker/daemon.rs` — boot logic now: try-load → if missing, `discover_and_enroll` → probe token → if 401 once, wipe + re-enroll → hand off to existing `connection::run`.
- `crates/transcoderr/src/main.rs` — coordinator branch starts `discovery::start_responder` after the listener binds (gated by `TRANSCODERR_DISCOVERY` env var); worker branch's clap default config path becomes `/var/lib/transcoderr/worker.toml`.

**No DB migration. No new dev-dependencies.**

---

## Task 1: Add `mdns-sd` dep + coordinator-side `discovery::start_responder`

Pure additive plumbing: a new module that wraps `mdns-sd::ServiceDaemon` and registers the `_transcoderr._tcp.local.` service. No call sites yet — Task 6 wires it into `main.rs`.

**Files:**
- Modify: `crates/transcoderr/Cargo.toml`
- Create: `crates/transcoderr/src/discovery/mod.rs`
- Modify: `crates/transcoderr/src/lib.rs`

- [ ] **Step 1: Branch verification + branch create**

```bash
git checkout main && git pull --ff-only
git checkout -b feat/worker-auto-discovery
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the `mdns-sd` dep**

In `crates/transcoderr/Cargo.toml`, in the `[dependencies]` table, add (alphabetically near `metrics`):

```toml
mdns-sd = "0.13"
```

- [ ] **Step 3: Create `discovery/mod.rs`**

Create `crates/transcoderr/src/discovery/mod.rs` with the following content:

```rust
//! Coordinator-side mDNS responder. Advertises
//! `_transcoderr._tcp.local.` so workers on the same LAN can find
//! us without operator-supplied config.
//!
//! TXT records: `enroll` (path), `ws` (path), `version` (informational).
//! Workers read `enroll` and `ws` directly; the version field is a
//! debugging aid for future protocol changes.

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceInfo};

/// Service type advertised by the coordinator and queried by workers.
pub const SERVICE_TYPE: &str = "_transcoderr._tcp.local.";

/// Build the `ServiceInfo` for our advertisement. Public so unit tests
/// can inspect it without actually starting a daemon.
pub fn build_service_info(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceInfo> {
    let host_name = format!("{}.local.", instance_name);
    let txt: Vec<(&str, &str)> = vec![
        ("enroll", "/api/worker/enroll"),
        ("ws", "/api/worker/connect"),
        ("version", env!("CARGO_PKG_VERSION")),
    ];
    // Empty `host_ipv4` means mdns-sd will auto-detect interfaces and
    // publish on all of them. That's what we want for a multi-homed host.
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        instance_name,
        &host_name,
        "",
        port,
        &txt[..],
    )
    .context("build mDNS ServiceInfo")?
    .enable_addr_auto();
    Ok(info)
}

/// Start the responder. Returned `ServiceDaemon` holds the registration
/// for its lifetime; drop it (or call `shutdown`) to unregister.
pub fn start_responder(
    port: u16,
    instance_name: &str,
) -> anyhow::Result<ServiceDaemon> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon")?;
    let info = build_service_info(port, instance_name)?;
    mdns.register(info).context("register mDNS service")?;
    tracing::info!(
        port,
        instance_name,
        service = SERVICE_TYPE,
        "mDNS responder started"
    );
    Ok(mdns)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_info_has_expected_shape() {
        let info = build_service_info(8765, "transcoderr-test").unwrap();
        assert_eq!(info.get_type(), SERVICE_TYPE);
        assert_eq!(info.get_port(), 8765);
        let props = info.get_properties();
        assert_eq!(
            props.get("enroll").and_then(|p| p.val_str()),
            Some("/api/worker/enroll")
        );
        assert_eq!(
            props.get("ws").and_then(|p| p.val_str()),
            Some("/api/worker/connect")
        );
        assert!(
            props.get("version").is_some(),
            "version TXT record should be present (informational)"
        );
    }

    #[test]
    fn instance_name_is_used_in_fullname() {
        let info = build_service_info(8765, "fluffy-coord").unwrap();
        let fullname = info.get_fullname();
        assert!(
            fullname.starts_with("fluffy-coord."),
            "fullname should start with the instance name; got {fullname}"
        );
    }
}
```

- [ ] **Step 4: Wire into the lib**

In `crates/transcoderr/src/lib.rs`, add the module declaration. Place it alphabetically near `db`/`dispatch` etc. (read the file first to find the right spot):

```rust
pub mod discovery;
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean compile (Cargo will fetch `mdns-sd` and its deps).

- [ ] **Step 6: Run the new unit tests**

```bash
cargo test -p transcoderr --lib discovery::tests 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 7: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/Cargo.toml \
        crates/transcoderr/src/discovery/mod.rs \
        crates/transcoderr/src/lib.rs \
        Cargo.lock
git commit -m "feat(discovery): coordinator-side mDNS responder module"
```

---

## Task 2: New `POST /api/worker/enroll` endpoint

Unauthenticated endpoint that mints a token, inserts a `kind='remote'` row, and returns `{id, secret_token, ws_url}`. Lives next to the existing `api/workers.rs` to mirror its style.

**Files:**
- Create: `crates/transcoderr/src/api/worker_enroll.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`
- Create: `crates/transcoderr/tests/worker_enroll.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create the handler module**

Create `crates/transcoderr/src/api/worker_enroll.rs`:

```rust
//! `POST /api/worker/enroll` — unauthenticated enrollment endpoint
//! used by workers that found the coordinator via mDNS.
//!
//! Trust model: open enrollment on the LAN. Documented in
//! `docs/superpowers/specs/2026-05-03-worker-auto-discovery-design.md`
//! (decision Q2-A). The endpoint is idempotent only in the sense that
//! repeated calls each insert a fresh row with a fresh token —
//! collisions on `name` are accepted (existing UI concern, not new).

use crate::db;
use crate::http::AppState;
use axum::{extract::State, http::StatusCode, Json};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct EnrollReq {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct EnrollResp {
    pub id: i64,
    /// Cleartext token. Returned exactly once at enrollment.
    pub secret_token: String,
    /// Pre-built WebSocket URL the worker should dial. Built from
    /// the coordinator's resolved `public_url` with the scheme flipped
    /// to `ws://` / `wss://`.
    pub ws_url: String,
}

pub async fn enroll(
    State(state): State<AppState>,
    Json(req): Json<EnrollReq>,
) -> Result<Json<EnrollResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let id = db::workers::insert_remote(&state.pool, &req.name, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "enroll: failed to insert worker row");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let ws_url = http_to_ws(&state.public_url) + "/api/worker/connect";

    tracing::info!(id, name = %req.name, "worker enrolled via auto-discovery");

    Ok(Json(EnrollResp { id, secret_token: token, ws_url }))
}

/// Flip `http://` → `ws://`, `https://` → `wss://`. Anything else passes
/// through (caller will see the malformed URL when it tries to dial).
fn http_to_ws(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_to_ws_handles_both_schemes() {
        assert_eq!(http_to_ws("http://192.168.1.50:8765"), "ws://192.168.1.50:8765");
        assert_eq!(http_to_ws("https://example.com"), "wss://example.com");
        // Trailing slash policy: caller appends "/api/worker/connect", so
        // we leave the trailing slash (or absence) alone.
        assert_eq!(http_to_ws("http://x/"), "ws://x/");
    }
}
```

- [ ] **Step 3: Wire the route into the public router**

Open `crates/transcoderr/src/api/mod.rs`. Read the current module list (near the top) and the `pub fn router(...)` body. Add:

After `pub mod workers;`:

```rust
pub mod worker_enroll;
```

In the `let public = Router::new()` block (the unauthenticated routes), after the existing `.route("/worker/connect", get(workers::connect))` line, add:

```rust
        .route("/worker/enroll", post(worker_enroll::enroll))
```

(The `post` import is already in scope at the top of the file via `use axum::routing::{delete, get, patch, post};`.)

- [ ] **Step 4: Create the integration test file**

Create `crates/transcoderr/tests/worker_enroll.rs`:

```rust
//! Integration tests for the unauthenticated `POST /api/worker/enroll`
//! endpoint. The endpoint is the server side of the worker
//! auto-discovery flow; the worker-side helper that calls it lives in
//! `worker::enroll` and is exercised end-to-end in `tests/auto_discovery.rs`.

mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn enroll_returns_token_and_inserts_row() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": "auto-1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "enroll should succeed unauthenticated");

    let body: serde_json::Value = resp.json().await.unwrap();
    let id = body["id"].as_i64().unwrap();
    let token = body["secret_token"].as_str().unwrap();
    let ws_url = body["ws_url"].as_str().unwrap();
    assert!(id > 0, "id must be positive: {id}");
    assert_eq!(token.len(), 64, "token must be 32-byte hex (64 chars): {token}");
    assert!(ws_url.starts_with("ws://"), "ws_url must use ws:// scheme: {ws_url}");
    assert!(ws_url.ends_with("/api/worker/connect"), "ws_url must point at /api/worker/connect: {ws_url}");

    // The row must exist and be `kind='remote'`.
    let row = transcoderr::db::workers::get_by_id(&app.pool, id)
        .await
        .unwrap()
        .expect("row exists");
    assert_eq!(row.kind, "remote");
    assert_eq!(row.name, "auto-1");
    assert_eq!(row.secret_token.as_deref(), Some(token));
}

#[tokio::test]
async fn enroll_rejects_empty_name() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn enroll_does_not_require_authentication() {
    // The endpoint must work with NO Authorization header AND no
    // session cookie — that's the whole point of auto-enrollment.
    let app = boot().await;
    let client = reqwest::Client::builder()
        .cookie_store(false)
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/api/worker/enroll", app.url))
        .json(&json!({"name": "no-auth"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
```

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Run the new tests**

```bash
cargo test -p transcoderr --test worker_enroll 2>&1 | tail -10
cargo test -p transcoderr --lib api::worker_enroll 2>&1 | tail -10
```

Expected: 3 passed (integration) + 1 passed (unit `http_to_ws_handles_both_schemes`).

- [ ] **Step 7: Regression net**

```bash
cargo test -p transcoderr --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/worker_enroll.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/tests/worker_enroll.rs
git commit -m "feat(api): POST /api/worker/enroll unauthenticated enrollment"
```

---

## Task 3: Worker-side `discovery::browse`

Symmetric to Task 1: a small wrapper over `mdns-sd::ServiceDaemon::browse` that returns the first matching coordinator within a 5 s deadline. The `mdns-sd` browse API surfaces results via a sync receiver; we drive it from a `tokio::task::spawn_blocking` so we don't tie up the runtime.

**Files:**
- Create: `crates/transcoderr/src/worker/discovery.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create `worker/discovery.rs`**

Create `crates/transcoderr/src/worker/discovery.rs`:

```rust
//! Worker-side mDNS browser. Used at boot when no `worker.toml`
//! exists: browses for `_transcoderr._tcp.local.` and returns the
//! first responder within a 5 s deadline.
//!
//! `mdns-sd` exposes a sync receiver; we drive it inside
//! `spawn_blocking` so we don't tie up the tokio runtime.

use anyhow::Context;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::time::Duration;

/// What a successful browse returns: enough to POST `/api/worker/enroll`.
#[derive(Debug, Clone)]
pub struct DiscoveredCoordinator {
    /// First IPv4 address advertised by the responder. We pick IPv4
    /// for now; if the only address is IPv6, we'll fall back to that
    /// in the same field (stored as a string).
    pub addr: String,
    pub port: u16,
    pub enroll_path: String,
    pub ws_path: String,
}

impl DiscoveredCoordinator {
    pub fn http_url(&self) -> String {
        format!("http://{}:{}", self.addr, self.port)
    }
    pub fn ws_url(&self) -> String {
        format!("ws://{}:{}{}", self.addr, self.port, self.ws_path)
    }
}

/// Browse for the first responder matching `_transcoderr._tcp.local.`.
/// Returns `Ok(None)` on timeout. `instance_filter`, when `Some`,
/// restricts results to instances whose fullname *contains* the given
/// substring — the integration test uses this to isolate concurrent runs.
pub async fn browse(
    deadline: Duration,
    instance_filter: Option<String>,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    tokio::task::spawn_blocking(move || browse_blocking(deadline, instance_filter))
        .await
        .context("mDNS browse task join")?
}

fn browse_blocking(
    deadline: Duration,
    instance_filter: Option<String>,
) -> anyhow::Result<Option<DiscoveredCoordinator>> {
    let mdns = ServiceDaemon::new().context("start mDNS daemon for browse")?;
    let receiver = mdns
        .browse(crate::discovery::SERVICE_TYPE)
        .context("start mDNS browse")?;

    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        let remaining = deadline.saturating_sub(start.elapsed());
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                if let Some(filter) = &instance_filter {
                    if !info.get_fullname().contains(filter) {
                        tracing::debug!(
                            fullname = info.get_fullname(),
                            filter = filter,
                            "skipping responder (instance filter mismatch)"
                        );
                        continue;
                    }
                }
                if let Some(parsed) = parse_service_info(&info) {
                    let _ = mdns.shutdown();
                    return Ok(Some(parsed));
                }
                tracing::warn!(
                    fullname = info.get_fullname(),
                    "found responder but TXT records were missing or malformed; ignoring"
                );
            }
            Ok(_other_event) => continue,
            Err(_timeout) => break,
        }
    }
    let _ = mdns.shutdown();
    Ok(None)
}

/// Pure helper: pull the address, port, and TXT records out of a
/// `ServiceInfo`. Returns `None` if any required field is missing.
/// Kept private but unit-testable.
fn parse_service_info(info: &ServiceInfo) -> Option<DiscoveredCoordinator> {
    let addrs = info.get_addresses();
    let addr = addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| addrs.iter().next())?
        .to_string();
    let port = info.get_port();
    let props = info.get_properties();
    let enroll_path = props.get("enroll")?.val_str()?.to_string();
    let ws_path = props.get("ws")?.val_str()?.to_string();
    Some(DiscoveredCoordinator { addr, port, enroll_path, ws_path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_when_addresses_unresolved() {
        // `build_service_info` uses `enable_addr_auto()` so addresses
        // are populated lazily by a running daemon. Without one, the
        // address set is empty and `parse_service_info` returns None
        // — that's the safe behavior we want at the boundary. The
        // populated case is covered by tests/auto_discovery.rs.
        let info = crate::discovery::build_service_info(8765, "test-instance").unwrap();
        assert!(parse_service_info(&info).is_none());
    }

    #[test]
    fn discovered_coordinator_url_helpers() {
        let d = DiscoveredCoordinator {
            addr: "192.168.1.50".into(),
            port: 8765,
            enroll_path: "/api/worker/enroll".into(),
            ws_path: "/api/worker/connect".into(),
        };
        assert_eq!(d.http_url(), "http://192.168.1.50:8765");
        assert_eq!(d.ws_url(), "ws://192.168.1.50:8765/api/worker/connect");
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/transcoderr/src/worker/mod.rs`, add (alphabetically near the existing `pub mod connection;` etc. — read the file first):

```rust
pub mod discovery;
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the new unit tests**

```bash
cargo test -p transcoderr --lib worker::discovery::tests 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 6: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/discovery.rs \
        crates/transcoderr/src/worker/mod.rs
git commit -m "feat(worker): mDNS browse helper"
```

---

## Task 4: Worker-side `enroll::request_and_write`

POSTs `/api/worker/enroll`, parses the response, and writes a `WorkerConfig`-shaped TOML to the cache path. Doesn't yet wire into `daemon::run` — that's Task 5.

**Files:**
- Create: `crates/transcoderr/src/worker/enroll.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create `worker/enroll.rs`**

Create `crates/transcoderr/src/worker/enroll.rs`:

```rust
//! Worker-side enrollment: discover the coordinator via mDNS,
//! POST `/api/worker/enroll`, write the resulting config to disk.
//!
//! Combines `worker::discovery::browse` and a single HTTP POST into
//! one operation. Called from `daemon::run` (Task 5) when no
//! `worker.toml` exists at boot.

use crate::worker::config::WorkerConfig;
use crate::worker::discovery::{browse, DiscoveredCoordinator};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const BROWSE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
struct EnrollReq<'a> {
    name: &'a str,
}

#[derive(Debug, Deserialize)]
struct EnrollResp {
    #[allow(dead_code)] // returned by server; logged but not used here
    id: i64,
    secret_token: String,
    ws_url: String,
}

/// Discover a coordinator on the LAN, enroll, and write the resulting
/// config to `cfg_path`. Used when no `worker.toml` exists.
///
/// `instance_filter`: when `Some`, restricts mDNS results to instances
/// whose fullname contains the given substring. Used by the integration
/// test to isolate concurrent runs; production callers pass `None`.
pub async fn discover_and_enroll(
    cfg_path: &Path,
    instance_filter: Option<String>,
) -> anyhow::Result<WorkerConfig> {
    let coord = browse(BROWSE_DEADLINE, instance_filter)
        .await?
        .ok_or_else(|| anyhow::anyhow!(
            "no coordinator found on the LAN within {BROWSE_DEADLINE:?} — \
             see docs/deploy.md for manual config"
        ))?;
    tracing::info!(addr = %coord.addr, port = coord.port, "discovered coordinator");

    let name = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unnamed-worker".into());

    let resp = post_enroll(&coord, &name).await?;
    write_config(cfg_path, &resp.ws_url, &resp.secret_token, &name)?;
    tracing::info!(path = %cfg_path.display(), "wrote auto-enrolled worker.toml");

    WorkerConfig::load(cfg_path).context("re-load freshly written worker.toml")
}

async fn post_enroll(
    coord: &DiscoveredCoordinator,
    name: &str,
) -> anyhow::Result<EnrollResp> {
    let url = format!("{}{}", coord.http_url(), coord.enroll_path);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&EnrollReq { name })
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_else(|_| "<no body>".into());
        anyhow::bail!("enroll {url} returned {status}: {body}");
    }
    resp.json::<EnrollResp>().await.context("parse enroll response")
}

/// Write a `worker.toml` at `path` with the given fields. Creates the
/// parent directory if missing.
pub fn write_config(
    path: &Path,
    coordinator_url: &str,
    coordinator_token: &str,
    name: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    let body = format!(
        "coordinator_url   = \"{coordinator_url}\"\n\
         coordinator_token = \"{coordinator_token}\"\n\
         name              = \"{name}\"\n"
    );
    std::fs::write(path, body)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_config_round_trips_through_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");
        write_config(
            &path,
            "ws://192.168.1.50:8765/api/worker/connect",
            "abcdef0123456789",
            "fluffy-1",
        )
        .unwrap();
        let cfg = WorkerConfig::load(&path).unwrap();
        assert_eq!(cfg.coordinator_url, "ws://192.168.1.50:8765/api/worker/connect");
        assert_eq!(cfg.coordinator_token, "abcdef0123456789");
        assert_eq!(cfg.name.as_deref(), Some("fluffy-1"));
    }

    #[test]
    fn write_config_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("worker.toml");
        write_config(&nested, "ws://x/api", "tok", "n").unwrap();
        assert!(nested.exists());
        // The contents must round-trip too.
        let cfg = WorkerConfig::load(&nested).unwrap();
        assert_eq!(cfg.coordinator_token, "tok");
    }
}
```

- [ ] **Step 3: Register the module**

In `crates/transcoderr/src/worker/mod.rs`, after the `pub mod discovery;` you added in Task 3, append:

```rust
pub mod enroll;
```

- [ ] **Step 4: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 5: Run the new unit tests**

```bash
cargo test -p transcoderr --lib worker::enroll::tests 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 6: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 7: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/enroll.rs \
        crates/transcoderr/src/worker/mod.rs
git commit -m "feat(worker): enroll module — discover + POST + write_config"
```

---

## Task 5: Wire discovery+enroll into worker boot path with 401 retry

The boot logic now handles four cases: (a) `worker.toml` exists and works, (b) missing → discover + enroll, (c) cached token rejected with 401 → wipe + re-discover (one retry), (d) discovery fails → exit with friendly error.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs` (add `probe_token`)
- Modify: `crates/transcoderr/src/worker/daemon.rs` (boot logic)
- Modify: `crates/transcoderr/src/main.rs` (clap default, wiring)

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add `probe_token` in connection.rs**

Open `crates/transcoderr/src/worker/connection.rs`. At the top, the existing imports should include `tokio_tungstenite` and `AUTHORIZATION`. Append (alongside the existing `connect_once` function) a small helper:

```rust
/// Outcome of a single WS-upgrade probe used at boot to detect
/// cached-token rejection before entering the long-lived reconnect
/// loop. The probe dials, classifies the response, and closes
/// immediately — no Register frame is exchanged.
#[derive(Debug)]
pub enum ProbeOutcome {
    Ok,
    Unauthorized,
    Other(anyhow::Error),
}

/// Single WS-upgrade attempt against `url` with `token` as the Bearer.
/// Used by `daemon::run` to detect a stale cached token (HTTP 401)
/// before falling into the infinite reconnect loop. On `Other`, we
/// can still enter the reconnect loop because the failure is likely
/// transient (DNS, TCP, TLS, etc.).
pub async fn probe_token(url: &str, token: &str) -> ProbeOutcome {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return ProbeOutcome::Other(anyhow::anyhow!("build request: {e}")),
    };
    let bearer = match format!("Bearer {token}").parse() {
        Ok(b) => b,
        Err(e) => return ProbeOutcome::Other(anyhow::anyhow!("build bearer header: {e}")),
    };
    req.headers_mut().insert(AUTHORIZATION, bearer);

    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => {
            let (mut sink, _) = ws.split();
            let _ = sink.send(WsMessage::Close(None)).await;
            ProbeOutcome::Ok
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(resp))
            if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED =>
        {
            ProbeOutcome::Unauthorized
        }
        Err(e) => ProbeOutcome::Other(anyhow::anyhow!("probe: {e}")),
    }
}
```

(The `futures::StreamExt` import for `ws.split()` should already be present at the top of the file from the existing `run` implementation. If not, add `use futures::StreamExt;` at the top of `probe_token`.)

- [ ] **Step 3: Extend `daemon::run` boot logic**

Open `crates/transcoderr/src/worker/daemon.rs`. Replace its current short body with the auto-recovery version. The new body:

```rust
//! Worker daemon entry point. Probes hardware, discovers installed
//! plugins, then hands off to `connection::run` which is the long-lived
//! reconnect loop.
//!
//! Boot order:
//!   1. Try to load `worker.toml`.
//!   2. If missing → run mDNS discovery + enrollment, write the file.
//!   3. Probe the WS upgrade once. If 401, wipe + re-enroll exactly
//!      once. If still 401, exit. Other errors fall through to the
//!      long-lived reconnect loop, which has its own backoff.

use crate::worker::config::WorkerConfig;
use crate::worker::connection::{probe_token, ProbeOutcome};
use std::path::PathBuf;

/// Run the worker daemon. Blocks forever (or exits the process via
/// `std::process::exit` on unrecoverable errors).
pub async fn run(cfg_path: PathBuf) -> ! {
    let cfg = match boot_config(&cfg_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "worker boot failed");
            std::process::exit(1);
        }
    };

    let name = cfg.resolved_name();
    tracing::info!(
        name = %name,
        coordinator = %cfg.coordinator_url,
        "starting worker daemon"
    );

    let caps = crate::ffmpeg_caps::FfmpegCaps::probe().await;
    let hw_caps = serde_json::json!({
        "has_libplacebo": caps.has_libplacebo,
    });

    let pool = match crate::db::open_in_memory().await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = ?e, "worker: failed to open in-memory sqlite for registry; aborting");
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        }
    };

    crate::steps::registry::init(
        pool.clone(),
        crate::hw::semaphores::DeviceRegistry::from_caps(&crate::hw::HwCaps::default()),
        std::sync::Arc::new(caps.clone()),
        Vec::new(),
    )
    .await;

    let ctx = crate::worker::connection::ConnectionContext {
        plugins_dir: std::path::PathBuf::from("./plugins"),
        coordinator_token: cfg.coordinator_token.clone(),
        name: name.clone(),
        hw_caps: hw_caps.clone(),
    };

    crate::worker::connection::run(
        cfg.coordinator_url,
        cfg.coordinator_token,
        ctx,
    )
    .await
}

/// Resolve a usable `WorkerConfig`, performing auto-discovery and 401
/// recovery if needed. Returns `Err` only on terminal failure.
async fn boot_config(cfg_path: &PathBuf) -> anyhow::Result<WorkerConfig> {
    let initial = match WorkerConfig::load(cfg_path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::info!(
                path = %cfg_path.display(),
                error = %e,
                "no usable worker.toml; running auto-discovery"
            );
            None
        }
    };

    let cfg = match initial {
        Some(c) => c,
        None => {
            crate::worker::enroll::discover_and_enroll(cfg_path, None).await?
        }
    };

    // Probe once to detect a stale cached token.
    match probe_token(&cfg.coordinator_url, &cfg.coordinator_token).await {
        ProbeOutcome::Ok => Ok(cfg),
        ProbeOutcome::Unauthorized => {
            tracing::warn!(
                "cached coordinator token rejected; deleting {} and re-running discovery",
                cfg_path.display()
            );
            let _ = std::fs::remove_file(cfg_path);
            let new_cfg = crate::worker::enroll::discover_and_enroll(cfg_path, None).await?;
            // Second probe — if STILL 401, give up.
            match probe_token(&new_cfg.coordinator_url, &new_cfg.coordinator_token).await {
                ProbeOutcome::Ok => Ok(new_cfg),
                ProbeOutcome::Unauthorized => Err(anyhow::anyhow!(
                    "freshly enrolled token was rejected with 401; refusing to loop"
                )),
                ProbeOutcome::Other(e) => {
                    // Transient — let the reconnect loop deal with it.
                    tracing::warn!(error = %e, "second probe failed; entering reconnect loop");
                    Ok(new_cfg)
                }
            }
        }
        ProbeOutcome::Other(e) => {
            tracing::warn!(
                error = %e,
                "initial probe failed; entering reconnect loop with current cfg"
            );
            Ok(cfg)
        }
    }
}
```

- [ ] **Step 4: Update `main.rs`**

Open `crates/transcoderr/src/main.rs`. Two changes:

(a) Change the clap default for `Cmd::Worker.config`. Find:

```rust
    Worker {
        #[arg(long, default_value = "worker.toml")]
        config: PathBuf,
    },
```

Replace with:

```rust
    Worker {
        /// Path to worker.toml. Default is the Docker-friendly
        /// /var/lib/transcoderr/worker.toml; override with `--config`
        /// for non-container or non-default deployments. If the file
        /// is missing on first boot, the worker auto-discovers a
        /// coordinator via mDNS and writes the file at this path.
        #[arg(long, default_value = "/var/lib/transcoderr/worker.toml")]
        config: PathBuf,
    },
```

(b) Change the `Cmd::Worker { config }` branch. Find:

```rust
        Cmd::Worker { config } => {
            let cfg = transcoderr::worker::config::WorkerConfig::load(&config)?;
            transcoderr::worker::daemon::run(cfg).await
        }
```

Replace with:

```rust
        Cmd::Worker { config } => {
            transcoderr::worker::daemon::run(config).await
        }
```

(`daemon::run` now takes the path and handles loading + discovery internally.)

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Worker-side regression net**

```bash
cargo test -p transcoderr --test worker_connect 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test plugin_remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test cancel_remote 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED. These tests don't go through `daemon::run`, but they do through `connection::run`, so a regression in `probe_token`'s imports would surface as a compile error.

- [ ] **Step 7: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs \
        crates/transcoderr/src/worker/daemon.rs \
        crates/transcoderr/src/main.rs
git commit -m "feat(worker): boot path auto-discovers + recovers from 401 once"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

Reason: this commit changes the worker boot path that every fresh worker container hits. A regression here means no worker can start without manual intervention. The user should review the diff before continuing.

---

## Task 6: Wire coordinator's discovery responder into `main.rs`

Adds the responder start-up to the coordinator's boot path. Gated by the `TRANSCODERR_DISCOVERY` env var (any value other than `"disabled"` enables it; default = enabled).

**Files:**
- Modify: `crates/transcoderr/src/main.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Start the responder after the listener binds**

Open `crates/transcoderr/src/main.rs`. Find the lines (in the `Cmd::Serve` branch):

```rust
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
```

Immediately AFTER that `tracing::info!` block, add:

```rust
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
```

The `let _mdns = ...` binding holds the `ServiceDaemon` for the lifetime of the function. Drop happens implicitly when `main` returns.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 4: Critical-path regression net**

Every coordinator-side integration test boots through `main.rs`'s codepath via `tests/common::boot()` — but `boot()` doesn't go through `main`, it builds the same `AppState` directly. So tests don't exercise the responder. Sanity check that nothing else regressed:

```bash
cargo test -p transcoderr --test concurrent_claim --test crash_recovery --test flow_engine 2>&1 | grep -E "FAILED|^test result" | tail -10
cargo test -p transcoderr --test remote_dispatch --test plugin_remote_dispatch --test cancel_remote 2>&1 | grep -E "FAILED|^test result" | tail -10
cargo test -p transcoderr --test worker_connect --test local_worker --test plugin_push --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -10
cargo test -p transcoderr --test worker_enroll 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/main.rs
git commit -m "feat(coordinator): start mDNS responder after listener binds"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

Reason: every test that boots the coordinator now (potentially) has a side effect of advertising on mDNS. Worth a glance before the integration test runs.

---

## Task 7: End-to-end integration test `tests/auto_discovery.rs`

The single end-to-end test that proves the wire is hot. Boots a coordinator, starts a discovery responder with a unique instance suffix, runs the worker-side enrollment routine filtered to that suffix, and asserts the row + `worker.toml` arrive intact.

**Files:**
- Create: `crates/transcoderr/tests/auto_discovery.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create the test file**

Create `crates/transcoderr/tests/auto_discovery.rs`:

```rust
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
    let _mdns = transcoderr::discovery::start_responder(port, &suffix)
        .expect("start responder");

    // Run the worker-side enrollment routine. Use the unique suffix
    // as an instance filter so we only ever resolve OUR responder.
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("worker.toml");
    let cfg = transcoderr::worker::enroll::discover_and_enroll(
        &cfg_path,
        Some(suffix.clone()),
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
        transcoderr::worker::enroll::discover_and_enroll(&cfg_path, Some(unique)),
    )
    .await
    .expect("must complete within 7s wall-clock");
    assert!(res.is_err(), "expected an error when no responder is present");
    assert!(!cfg_path.exists(), "no partial file may be written");
}
```

- [ ] **Step 3: Run the new tests**

```bash
cargo test -p transcoderr --test auto_discovery 2>&1 | tail -15
```

Expected: 2 passed. The first test should complete in ~1–2 s; the second takes ~5–7 s due to the discovery timeout.

If the first test hangs:
- Most likely the OS firewall is dropping multicast on loopback. Confirm by checking `RUST_LOG=mdns_sd=debug cargo test -- --nocapture`. On macOS, `mdns-sd` does work over loopback by default; if this is a CI sandbox issue, mark the test `#[ignore]` and document the manual run.
- Less likely: a real transcoderr coordinator is running on the dev machine and (somehow) bypassed the instance filter. Add a print to confirm the resolved fullname.

If the second test fails (`res` is `Ok`):
- Some other transcoderr coordinator on the LAN is responding. Either kill it or run the test on a quieter network. The unique-suffix instance filter should prevent this in normal test runs.

- [ ] **Step 4: Run the full integration suite for confidence**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/worker-auto-discovery" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/auto_discovery.rs
git commit -m "test(auto_discovery): end-to-end mDNS browse + enroll suite"
```

---

## Self-Review Notes

Spec coverage check, section by section:

| Spec section | Task |
|---|---|
| Coordinator advertises `_transcoderr._tcp.local.` with TXT records | Task 1 |
| New unauthenticated `POST /api/worker/enroll`, returns `{id, secret_token, ws_url}` | Task 2 |
| Worker-side mDNS browse with 5 s deadline | Task 3 |
| Worker-side enroll + write `worker.toml` | Task 4 |
| Worker boot path: try-load → discover → enroll → connect | Task 5 |
| 401 with cached token → wipe + re-enroll once | Task 5 (`boot_config`) |
| Discovery times out → exit with friendly error | Task 4 (`discover_and_enroll` returns Err) + Task 5 (caller exits 1) |
| Disable switch `TRANSCODERR_DISCOVERY=disabled` | Task 6 |
| Default config path `/var/lib/transcoderr/worker.toml` | Task 5 (clap default) |
| `discovery::publish` unit test | Task 1 (`service_info_has_expected_shape`) |
| `enroll::write_config` round-trip unit test | Task 4 (`write_config_round_trips_through_load`) |
| `POST /api/worker/enroll` handler test | Task 2 (`tests/worker_enroll.rs`) |
| End-to-end integration test | Task 7 |

Cross-task type/signature consistency:

- `discovery::SERVICE_TYPE: &str = "_transcoderr._tcp.local."` — declared in Task 1, consumed in Task 3 (`worker::discovery::browse`).
- `EnrollResp { id, secret_token, ws_url }` — defined in Task 2 (server), mirrored in Task 4 (worker client).
- `WorkerConfig` shape (`coordinator_url`, `coordinator_token`, `name`) — unchanged from existing code; Task 4's `write_config` produces a TOML that matches.
- `DiscoveredCoordinator { addr, port, enroll_path, ws_path }` — Task 3 produces, Task 4 consumes.
- `ProbeOutcome::{Ok, Unauthorized, Other}` — Task 5 (connection.rs), consumed by Task 5's `boot_config` in daemon.rs.

No placeholders. Every step has executable code or exact commands. All file paths absolute. Bite-sized step granularity. Frequent commits — 7 total commits, one per task. Single-PR feature.

Pause checkpoints set at Tasks 5 and 6 (worker boot path + coordinator main.rs) per the brainstorm guidance.
