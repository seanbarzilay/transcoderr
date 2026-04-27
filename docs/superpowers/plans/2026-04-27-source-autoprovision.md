# Source Auto-Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the manual "operator pastes webhook URL into Radarr/Sonarr/Lidarr" UX with auto-provisioning. Source create/update/delete + boot-time reconcile call the *arr's `/api/v3/notification` endpoint to keep the *arr-side webhook in sync with transcoderr's local source rows.

**Architecture:** New `arr` module wraps the *arr REST API. New `public_url` resolver gives transcoderr its own reachable URL (env var with sensible default). Four lifecycle hooks attach to the source endpoints: create provisions the *arr webhook, update reprovisions on URL/key change, delete tears down, boot reconcile drift-checks. WebUI source form gains kind-conditional fields. Legacy v0.9.x sources untouched (manual flow preserved by the absence of `arr_notification_id` in their config).

**Tech Stack:** Rust workspace (`crates/transcoderr` + `crates/transcoderr-api-types`), `reqwest` for the *arr HTTP client, `wiremock` for tests, `hostname` crate for default URL, React + TypeScript + react-query in `web/`.

**Spec:** `docs/superpowers/specs/2026-04-27-source-autoprovision-design.md`

---

## File Structure

```
Cargo.toml                                                    [modify: hostname workspace dep, wiremock workspace dev-dep]
crates/transcoderr/Cargo.toml                                 [modify: pull deps from workspace]
crates/transcoderr/src/arr/mod.rs                             [create: Kind + Notification + Client]
crates/transcoderr/src/arr/reconcile.rs                       [create: boot reconciler]
crates/transcoderr/src/lib.rs                                 [modify: pub mod arr; pub mod public_url;]
crates/transcoderr/src/public_url.rs                          [create: resolve + PublicUrl]
crates/transcoderr/src/http/mod.rs                            [modify: AppState gains public_url field]
crates/transcoderr/src/main.rs                                [modify: resolve public_url at boot, spawn reconciler]
crates/transcoderr/src/db/sources.rs                          [modify: add list/get_by_id/update_arr_notification_id helpers]
crates/transcoderr/src/api/sources.rs                         [modify: auto-provision create / delete / update + redaction]
crates/transcoderr/tests/auto_provision.rs                    [create: wiremock integration test for create-source flow]
web/src/pages/sources.tsx                                     [modify: kind-conditional form + auto/manual badge]
```

---

## Task 1: Workspace deps for `hostname` + `wiremock`

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/transcoderr/Cargo.toml`

`wiremock` is a dev-dep used by Tasks 3, 4, 8 for HTTP mocking. `hostname` is a runtime dep used by Task 5 for the public URL default.

- [ ] **Step 1: Add workspace dependencies**

In `Cargo.toml` (workspace root), find the `[workspace.dependencies]` block and append:

```toml
hostname = "0.4"
```

The `[workspace.dependencies]` block doesn't typically include dev-deps. Add `wiremock` directly to the transcoderr crate's dev-dependencies in step 2.

- [ ] **Step 2: Pull `hostname` into transcoderr; add `wiremock` as dev-dep**

In `crates/transcoderr/Cargo.toml`, find the existing `[dependencies]` block and append:

```toml
hostname = { workspace = true }
```

Then find the `[dev-dependencies]` block and append:

```toml
wiremock = "0.6"
```

- [ ] **Step 3: Verify build still works**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: clean build (the new deps download but nothing uses them yet).

Run: `cargo build --workspace --locked 2>&1 | tail -3`
Expected: clean build with the refreshed lockfile.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add Cargo.toml Cargo.lock crates/transcoderr/Cargo.toml
git commit -m "build: add hostname (runtime) + wiremock (dev) for arr client"
```

---

## Task 2: `arr` module — types and `Client` skeleton

**Files:**
- Create: `crates/transcoderr/src/arr/mod.rs`
- Modify: `crates/transcoderr/src/lib.rs` (add `pub mod arr;`)

This task adds the type system and the `Client::new` constructor. HTTP methods come in Tasks 3 and 4. Splitting lets us land the Kind-parsing tests first cleanly.

- [ ] **Step 1: Create the module**

Create `crates/transcoderr/src/arr/mod.rs` with:

```rust
//! Typed client for Radarr / Sonarr / Lidarr's `/api/v3/notification`
//! webhook-management endpoint. All three are servarr forks and share
//! the same JSON shape. `Kind` discriminates which event flags to
//! enable on create.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Radarr,
    Sonarr,
    Lidarr,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "radarr" => Some(Kind::Radarr),
            "sonarr" => Some(Kind::Sonarr),
            "lidarr" => Some(Kind::Lidarr),
            _ => None,
        }
    }
}

/// Subset of the *arr Notification model we care about. Other fields
/// (id, includeHealth, tags, etc.) are deserialized via `#[serde(flatten)]`
/// into `extra` so we round-trip them on update without dropping
/// operator-set values.
#[derive(Debug, Serialize, Deserialize)]
pub struct Notification {
    pub id: i64,
    pub name: String,
    pub implementation: String,
    #[serde(rename = "configContract")]
    pub config_contract: String,
    pub fields: Vec<Field>,
    #[serde(default, rename = "onGrab")]
    pub on_grab: bool,
    #[serde(default, rename = "onDownload")]
    pub on_download: bool,
    #[serde(default, rename = "onUpgrade")]
    pub on_upgrade: bool,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub value: serde_json::Value,
}

pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl Client {
    /// Construct a client. Trims trailing `/` from `base_url` so callers
    /// can pass either form. 15-second per-request timeout — generous
    /// for typical homelab latencies, tight enough that an unreachable
    /// *arr fails fast.
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .context("building reqwest client")?,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parse_known_kinds() {
        assert_eq!(Kind::parse("radarr"), Some(Kind::Radarr));
        assert_eq!(Kind::parse("sonarr"), Some(Kind::Sonarr));
        assert_eq!(Kind::parse("lidarr"), Some(Kind::Lidarr));
    }

    #[test]
    fn kind_parse_rejects_other_strings() {
        assert_eq!(Kind::parse("generic"), None);
        assert_eq!(Kind::parse("webhook"), None);
        assert_eq!(Kind::parse(""), None);
        assert_eq!(Kind::parse("RADARR"), None); // case-sensitive
    }

    #[test]
    fn client_new_trims_trailing_slash() {
        let c = Client::new("http://radarr:7878/", "k").unwrap();
        assert_eq!(c.base_url, "http://radarr:7878");
        let c = Client::new("http://radarr:7878", "k").unwrap();
        assert_eq!(c.base_url, "http://radarr:7878");
    }
}
```

- [ ] **Step 2: Wire the module into the crate**

In `crates/transcoderr/src/lib.rs`, find the existing `pub mod` declarations. Add:

```rust
pub mod arr;
```

Place it alphabetically, or near the other module declarations of similar type.

- [ ] **Step 3: Build and run tests**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test -p transcoderr --lib arr:: 2>&1 | tail -10`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/arr/mod.rs crates/transcoderr/src/lib.rs
git commit -m "feat(arr): types + Client skeleton for *arr notification API"
```

---

## Task 3: `arr::Client::create_notification` + `event_flags` helper

**Files:**
- Modify: `crates/transcoderr/src/arr/mod.rs`

Add the create method + the per-kind event flag helper + a wiremock test.

- [ ] **Step 1: Append the method and helper**

In `crates/transcoderr/src/arr/mod.rs`, inside the `impl Client { ... }` block (after `pub fn new`), append:

```rust
    /// Create a Webhook notification on the *arr. Returns the created
    /// Notification (with the *arr-assigned `id`). On 4xx/5xx, the error
    /// chain includes the *arr's response body so operators see the
    /// actual reason (e.g. `Unauthorized`, `Invalid api key`).
    pub async fn create_notification(
        &self,
        kind: Kind,
        name: &str,
        webhook_url: &str,
        secret: &str,
    ) -> Result<Notification> {
        let mut body = serde_json::json!({
            "name": format!("transcoderr-{name}"),
            "implementation": "Webhook",
            "configContract": "WebhookSettings",
            "fields": [
                { "name": "url",      "value": webhook_url },
                { "name": "method",   "value": 1 },
                { "name": "username", "value": "" },
                { "name": "password", "value": secret },
            ],
        });
        // Splice per-kind event flags into the body.
        if let Some(map) = body.as_object_mut() {
            for (flag, val) in event_flags(kind) {
                map.insert(flag.into(), serde_json::Value::Bool(val));
            }
        }

        let url = format!("{}/api/v3/notification", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("X-Api-Key", &self.api_key)
            .json(&body)
            .send()
            .await
            .context("posting *arr notification")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<Notification>()
            .await
            .context("parsing *arr response")
    }
}

/// Per-kind event flags. Radarr fires onGrab/onDownload/onUpgrade;
/// Sonarr adds onSeriesAdd / onEpisodeFileDelete; Lidarr's are
/// album/artist-flavored. We default to the most useful subset for
/// transcoderr's "react to a downloaded file" use case.
fn event_flags(kind: Kind) -> Vec<(&'static str, bool)> {
    match kind {
        Kind::Radarr => vec![
            ("onGrab", false),
            ("onDownload", true),
            ("onUpgrade", true),
        ],
        Kind::Sonarr => vec![
            ("onGrab", false),
            ("onDownload", true),
            ("onUpgrade", true),
            ("onSeriesAdd", false),
            ("onEpisodeFileDelete", false),
        ],
        Kind::Lidarr => vec![
            ("onGrab", false),
            ("onReleaseImport", true),
            ("onUpgrade", true),
        ],
    }
}
```

- [ ] **Step 2: Add a wiremock test**

In the same file, append inside `mod tests` (before the closing `}`):

```rust
    use serde_json::Value;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn create_notification_builds_correct_payload_and_returns_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/notification"))
            .and(header("X-Api-Key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "name": "transcoderr-Movies",
                "implementation": "Webhook",
                "configContract": "WebhookSettings",
                "fields": [
                    {"name": "url", "value": "http://transcoderr:8099/webhook/radarr"},
                    {"name": "password", "value": "abc123"},
                ],
                "onGrab": false,
                "onDownload": true,
                "onUpgrade": true,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), "test-key").unwrap();
        let n = client
            .create_notification(
                Kind::Radarr,
                "Movies",
                "http://transcoderr:8099/webhook/radarr",
                "abc123",
            )
            .await
            .unwrap();
        assert_eq!(n.id, 42);
        assert_eq!(n.name, "transcoderr-Movies");

        // Verify the request body shape via the mock's recorded request.
        let received = &server.received_requests().await.unwrap()[0];
        let body: Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(body["implementation"], "Webhook");
        let fields = body["fields"].as_array().unwrap();
        let pw = fields.iter().find(|f| f["name"] == "password").unwrap();
        assert_eq!(pw["value"], "abc123");
        assert_eq!(body["onDownload"], true);
    }

    #[tokio::test]
    async fn create_notification_surfaces_arr_error_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/notification"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "message": "Unauthorized"
            })))
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), "wrong-key").unwrap();
        let err = client
            .create_notification(Kind::Radarr, "Movies", "http://x/webhook", "s")
            .await
            .unwrap_err();
        let s = format!("{err:?}");
        assert!(s.contains("401"), "got {s}");
        assert!(s.contains("Unauthorized"), "got {s}");
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib create_notification 2>&1 | tail -15`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/arr/mod.rs
git commit -m "feat(arr): Client::create_notification with per-kind event flags"
```

---

## Task 4: `arr::Client` — `list`, `get`, `delete`

**Files:**
- Modify: `crates/transcoderr/src/arr/mod.rs`

Three more HTTP methods. `get_notification` returns `Ok(None)` on 404 (used by the boot reconciler to detect a missing webhook).

- [ ] **Step 1: Append the methods**

In `crates/transcoderr/src/arr/mod.rs`, inside the `impl Client { ... }` block (after `create_notification`), append:

```rust
    pub async fn list_notifications(&self) -> Result<Vec<Notification>> {
        let url = format!("{}/api/v3/notification", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("listing *arr notifications")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<Vec<Notification>>()
            .await
            .context("parsing *arr response")
    }

    /// Fetch a single notification by id. 404 → Ok(None), used by the
    /// boot reconciler to distinguish "drifted" from "missing".
    pub async fn get_notification(&self, id: i64) -> Result<Option<Notification>> {
        let url = format!("{}/api/v3/notification/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("getting *arr notification")?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        Ok(Some(
            resp.json::<Notification>()
                .await
                .context("parsing *arr response")?,
        ))
    }

    pub async fn delete_notification(&self, id: i64) -> Result<()> {
        let url = format!("{}/api/v3/notification/{id}", self.base_url);
        let resp = self
            .http
            .delete(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("deleting *arr notification")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        Ok(())
    }
```

- [ ] **Step 2: Add wiremock tests**

Append inside `mod tests`:

```rust
    #[tokio::test]
    async fn delete_notification_passes_id_in_path() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/api/v3/notification/42"))
            .and(header("X-Api-Key", "k"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), "k").unwrap();
        client.delete_notification(42).await.unwrap();
    }

    #[tokio::test]
    async fn get_notification_returns_none_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/notification/99"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), "k").unwrap();
        let result = client.get_notification(99).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_notification_returns_some_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/notification/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 7,
                "name": "transcoderr-Movies",
                "implementation": "Webhook",
                "configContract": "WebhookSettings",
                "fields": [],
                "onDownload": true,
            })))
            .mount(&server)
            .await;

        let client = Client::new(&server.uri(), "k").unwrap();
        let n = client.get_notification(7).await.unwrap().unwrap();
        assert_eq!(n.id, 7);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib arr:: 2>&1 | tail -15`
Expected: 8 arr tests pass total (3 from Task 2 + 2 from Task 3 + 3 from this task).

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/arr/mod.rs
git commit -m "feat(arr): Client::list/get/delete_notification with 404 → None"
```

---

## Task 5: `public_url::resolve` helper

**Files:**
- Create: `crates/transcoderr/src/public_url.rs`
- Modify: `crates/transcoderr/src/lib.rs` (add `pub mod public_url;`)

- [ ] **Step 1: Create the module**

Create `crates/transcoderr/src/public_url.rs` with:

```rust
//! Resolve the URL that *arr instances should use to reach this
//! transcoderr server. Set once at boot, stored in AppState, baked
//! into webhook configurations on source-create.

use std::net::SocketAddr;

#[derive(Debug, Clone, Copy)]
pub enum Source {
    Env,
    Default,
}

#[derive(Debug, Clone)]
pub struct PublicUrl {
    pub url: String,
    pub source: Source,
}

/// Resolve from `TRANSCODERR_PUBLIC_URL` if set, else
/// `http://{gethostname()}:{addr.port()}`. Falls back to `localhost`
/// if the gethostname() syscall fails (extremely rare).
pub fn resolve(bound_addr: SocketAddr) -> PublicUrl {
    if let Ok(url) = std::env::var("TRANSCODERR_PUBLIC_URL") {
        let url = url.trim_end_matches('/').to_string();
        return PublicUrl {
            url,
            source: Source::Env,
        };
    }
    let host = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".to_string());
    let url = format!("http://{host}:{}", bound_addr.port());
    PublicUrl {
        url,
        source: Source::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn addr() -> SocketAddr {
        "127.0.0.1:8099".parse().unwrap()
    }

    #[test]
    fn resolve_uses_env_var_when_set() {
        // serial_test ensures env-var tests don't race. The repo's
        // existing dev-deps include serial_test.
        std::env::set_var("TRANSCODERR_PUBLIC_URL", "https://t.example.com/");
        let p = resolve(addr());
        std::env::remove_var("TRANSCODERR_PUBLIC_URL");
        assert_eq!(p.url, "https://t.example.com");
        assert!(matches!(p.source, Source::Env));
    }

    #[test]
    fn resolve_defaults_to_hostname_and_bound_port() {
        std::env::remove_var("TRANSCODERR_PUBLIC_URL");
        let p = resolve(addr());
        assert!(matches!(p.source, Source::Default));
        // The actual hostname depends on the test host — assert the
        // shape rather than the literal. URL must start with http://
        // and end with the bound port.
        assert!(p.url.starts_with("http://"), "got {}", p.url);
        assert!(p.url.ends_with(":8099"), "got {}", p.url);
    }
}
```

NOTE: the env-var tests should be marked `#[serial_test::serial]` if multiple tests touch `TRANSCODERR_PUBLIC_URL`; for two tests in the same file this is generally fine because `cargo test` defaults to running tests in the same binary serially within a single thread group, but to be safe add the attribute. If `serial_test` isn't already imported in this file, add `use serial_test::serial;` and `#[serial]` to both test fns.

Quick check — confirm `serial_test = "3"` is in `crates/transcoderr/Cargo.toml`'s `[dev-dependencies]`. (Per the explore output, it is.) If `serial_test` isn't in scope when the test module compiles, drop the `#[serial]` annotations and the tests will still pass when `cargo test` is single-threaded.

- [ ] **Step 2: Wire the module**

In `crates/transcoderr/src/lib.rs`, add:

```rust
pub mod public_url;
```

Alphabetically near the other `pub mod` declarations.

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib public_url:: 2>&1 | tail -10`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/public_url.rs crates/transcoderr/src/lib.rs
git commit -m "feat(public-url): resolve TRANSCODERR_PUBLIC_URL with hostname fallback"
```

---

## Task 6: AppState plumbing for `public_url`

**Files:**
- Modify: `crates/transcoderr/src/http/mod.rs`
- Modify: `crates/transcoderr/src/main.rs`
- Modify: `crates/transcoderr/tests/common/mod.rs` (test boot helper)

The reconciler and the source-create handler both need `public_url`. We thread it through `AppState` (HTTP handlers) and pass a clone to the reconciler when we spawn it (Task 11). The test boot helper needs to seed a stub URL.

- [ ] **Step 1: Add the field to `AppState`**

In `crates/transcoderr/src/http/mod.rs`, find the `pub struct AppState { ... }` and append a new field:

```rust
    pub public_url: std::sync::Arc<String>,
```

- [ ] **Step 2: Resolve and inject in `serve`**

In `crates/transcoderr/src/main.rs`, find the section where `AppState` is constructed and the listener is bound. There's currently:

```rust
let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
let addr = listener.local_addr()?;
tracing::info!(addr = %addr, "serving");

let serve = axum::serve(listener, app).with_graceful_shutdown(...);
```

Reorder so `public_url::resolve` runs BEFORE `AppState` is constructed (because AppState needs the resolved URL). The bind must happen first to know `addr`. So the new shape is:

```rust
let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
let addr = listener.local_addr()?;
let public_url = transcoderr::public_url::resolve(addr);
tracing::info!(
    public_url = %public_url.url,
    source = ?public_url.source,
    addr = %addr,
    "transcoderr serving",
);
let public_url_arc = std::sync::Arc::new(public_url.url);

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
};

let app = transcoderr::http::router(state);

let serve = axum::serve(listener, app).with_graceful_shutdown(
    async move {
        let _ = tokio::signal::ctrl_c().await;
    },
);
```

This may require moving the existing `let app = ...; let state = ...;` lines into the order shown. Look for the `transcoderr::http::router(state)` call site and adjust.

- [ ] **Step 3: Update the test boot helper**

In `crates/transcoderr/tests/common/mod.rs`, find the `AppState { ... }` literal in `boot()` and add the new field:

```rust
    public_url: std::sync::Arc::new("http://test:8099".to_string()),
```

- [ ] **Step 4: Build everything**

Run: `cargo build --workspace 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`. Pre-existing metrics flake notwithstanding.

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/http/mod.rs crates/transcoderr/src/main.rs crates/transcoderr/tests/common/mod.rs
git commit -m "feat(http): AppState gains public_url; resolved at serve()"
```

---

## Task 7: New `db::sources` helpers

**Files:**
- Modify: `crates/transcoderr/src/db/sources.rs`

The boot reconciler needs `list_all`, `get_by_id`, and `update_arr_notification_id`. None of these exist today (api/sources.rs uses inline SQL). We add them to `db::sources` for the reconciler to use; the existing api/sources.rs callers can continue to use inline SQL (refactoring those is out of scope for this branch).

- [ ] **Step 1: Add the helpers**

In `crates/transcoderr/src/db/sources.rs`, append (after the existing `get_webhook_by_name_and_token`):

```rust
pub async fn list_all(pool: &SqlitePool) -> anyhow::Result<Vec<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY id")
        .fetch_all(pool)
        .await?)
}

pub async fn get_by_id(pool: &SqlitePool, id: i64) -> anyhow::Result<Option<SourceRow>> {
    Ok(sqlx::query_as("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Update only the `arr_notification_id` field within an existing source's
/// `config_json`. Used by the boot reconciler when a webhook drifted and
/// got recreated under a new id.
pub async fn update_arr_notification_id(
    pool: &SqlitePool,
    source_id: i64,
    new_id: i64,
) -> anyhow::Result<()> {
    let row = get_by_id(pool, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("source {source_id} not found"))?;
    let mut cfg: serde_json::Value = serde_json::from_str(&row.config_json).unwrap_or_default();
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("arr_notification_id".into(), serde_json::json!(new_id));
    }
    let cfg_str = serde_json::to_string(&cfg)?;
    sqlx::query("UPDATE sources SET config_json = ? WHERE id = ?")
        .bind(cfg_str)
        .bind(source_id)
        .execute(pool)
        .await?;
    Ok(())
}
```

- [ ] **Step 2: Build and test**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -10`
Expected: existing tests still pass (no new tests in this task — the helpers are exercised by Task 11's reconciler, which has its own tests).

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/db/sources.rs
git commit -m "feat(db): list_all / get_by_id / update_arr_notification_id helpers"
```

---

## Task 8: `api/sources::create` auto-provision branch

**Files:**
- Modify: `crates/transcoderr/src/api/sources.rs`
- Create: `crates/transcoderr/tests/auto_provision.rs`

Refactor the `create` handler to branch on kind. For radarr/sonarr/lidarr, call the *arr to provision the webhook before persisting locally. Generate the secret_token server-side (operator never sees it). Add a wiremock-backed integration test.

- [ ] **Step 1: Replace the `create` handler body**

In `crates/transcoderr/src/api/sources.rs`, find the `pub async fn create(...)` function. Replace its body with the auto-provision branch logic. The exact replacement:

```rust
pub async fn create(
    State(state): State<crate::http::AppState>,
    Json(req): Json<CreateSourceReq>,
) -> Result<Json<CreatedIdResp>, (StatusCode, Json<ApiError>)> {
    use rand::RngCore;

    if let Some(arr_kind) = crate::arr::Kind::parse(&req.kind) {
        // Auto-provision path.
        let cfg = req.config.as_object().ok_or_else(|| {
            (StatusCode::BAD_REQUEST, Json(ApiError::new("validation.bad_request",
                "config must be an object with base_url and api_key")))
        })?;
        let base_url = cfg.get("base_url").and_then(|v| v.as_str()).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, Json(ApiError::new("validation.bad_request",
                "config.base_url is required for radarr/sonarr/lidarr")))
        })?;
        let api_key = cfg.get("api_key").and_then(|v| v.as_str()).ok_or_else(|| {
            (StatusCode::BAD_REQUEST, Json(ApiError::new("validation.bad_request",
                "config.api_key is required for radarr/sonarr/lidarr")))
        })?;

        // Generate a 32-byte hex secret. The operator never sees it; it
        // lives between transcoderr and the *arr.
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let secret_token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

        let webhook_url = format!("{}/webhook/{}", state.public_url, req.kind);
        let client = crate::arr::Client::new(base_url, api_key).map_err(|e| {
            (StatusCode::BAD_GATEWAY, Json(ApiError::new("arr.client",
                &format!("failed to construct *arr client: {e}"))))
        })?;
        let notification = client
            .create_notification(arr_kind, &req.name, &webhook_url, &secret_token)
            .await
            .map_err(|e| {
                (StatusCode::BAD_GATEWAY, Json(ApiError::new("arr.create_notification",
                    &format!("failed to create webhook on {}: {e}", req.kind))))
            })?;

        // Persist locally with the *arr-assigned id stamped into config.
        let mut cfg_out = req.config.clone();
        if let Some(obj) = cfg_out.as_object_mut() {
            obj.insert("arr_notification_id".into(), serde_json::json!(notification.id));
        }
        let id = crate::db::sources::insert(&state.pool, &req.kind, &req.name, &cfg_out, &secret_token)
            .await
            .map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError::new("db.insert",
                    &format!("failed to persist source: {e}"))))
            })?;
        return Ok(Json(CreatedIdResp { id }));
    }

    // Manual path (generic / webhook): unchanged behavior.
    let id = crate::db::sources::insert(&state.pool, &req.kind, &req.name, &req.config, &req.secret_token)
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError::new("db.insert",
                &format!("failed to persist source: {e}"))))
        })?;
    Ok(Json(CreatedIdResp { id }))
}
```

NOTE: the exact error-response shape (`StatusCode + Json<ApiError>`) must match what the existing handlers return. If the existing return type is `Result<Json<...>, StatusCode>` (no ApiError body), use that simpler form: map *arr errors to `StatusCode::BAD_GATEWAY` and let the body be empty. Look at the existing function signature and match it. If the codebase has a custom `ApiError` type wired into `IntoResponse`, use that instead.

The `rand::RngCore` import requires `rand` to be in scope. Check if it's already a dep of transcoderr; if not, the `[dependencies]` block needs `rand = "0.8"`. Per the explore output the transcoderr crate already uses `rand = "0.8"` (line 33 of Cargo.toml).

- [ ] **Step 2: Add a redaction helper for `api_key`**

In the same file, find the existing list/get handlers that compute `secret_token: if auth == AuthSource::Token { "***".into() } else { secret }`. Extend the redaction to also redact `api_key` when present in `config`:

Add this helper at module top (after the existing `use` lines):

```rust
fn redact_config(config: &serde_json::Value, redact: bool) -> serde_json::Value {
    if !redact {
        return config.clone();
    }
    let mut out = config.clone();
    if let Some(obj) = out.as_object_mut() {
        if obj.contains_key("api_key") {
            obj.insert("api_key".into(), serde_json::json!("***"));
        }
    }
    out
}
```

Then in the existing `list` / `get` handlers, replace `config: serde_json::from_str(&config_str).unwrap_or_default()` with `config: redact_config(&serde_json::from_str(&config_str).unwrap_or_default(), auth == AuthSource::Token)`.

- [ ] **Step 3: Create the integration test**

Create `crates/transcoderr/tests/auto_provision.rs`:

```rust
//! Integration test for the auto-provision create-source flow. Spins
//! up wiremock as a fake Radarr; confirms transcoderr POSTs to
//! /api/v3/notification before persisting the source row.

mod common;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn create_source_radarr_calls_arr_then_persists() {
    let arr = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/notification"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 42,
            "name": "transcoderr-Movies",
            "implementation": "Webhook",
            "configContract": "WebhookSettings",
            "fields": [],
            "onDownload": true,
        })))
        .expect(1)
        .mount(&arr)
        .await;

    let app = common::boot().await;
    let client = reqwest::Client::new();

    let resp: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "radarr",
            "name": "Movies",
            "config": {
                "base_url": arr.uri(),
                "api_key": "test-key",
            },
            "secret_token": ""  // ignored on auto-provision path
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let id = resp["id"].as_i64().unwrap();

    // Confirm the row landed with arr_notification_id stamped in.
    let detail: serde_json::Value = client
        .get(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["kind"], "radarr");
    assert_eq!(detail["name"], "Movies");
    assert_eq!(detail["config"]["arr_notification_id"], 42);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p transcoderr --test auto_provision 2>&1 | tail -10`
Expected: 1 test passes.

Run wider check:

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`. Pre-existing metrics flake notwithstanding.

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/api/sources.rs crates/transcoderr/tests/auto_provision.rs
git commit -m "feat(api): auto-provision *arr webhook on POST /api/sources"
```

---

## Task 9: `api/sources::delete` symmetric teardown

**Files:**
- Modify: `crates/transcoderr/src/api/sources.rs`

- [ ] **Step 1: Extend the `delete` handler**

In `crates/transcoderr/src/api/sources.rs`, find the existing `delete` handler. Today it just deletes the row. Replace its body with:

```rust
pub async fn delete(
    State(state): State<crate::http::AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    // Fetch first so we know what to clean up remotely.
    let row = match crate::db::sources::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let cfg: serde_json::Value =
        serde_json::from_str(&row.config_json).unwrap_or_default();
    let arr_kind = crate::arr::Kind::parse(&row.kind);
    let notification_id = cfg.get("arr_notification_id").and_then(|v| v.as_i64());

    if let (Some(_arr_kind), Some(notification_id)) = (arr_kind, notification_id) {
        let base_url = cfg.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
        let api_key = cfg.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
        if !base_url.is_empty() && !api_key.is_empty() {
            match crate::arr::Client::new(base_url, api_key) {
                Ok(client) => match client.delete_notification(notification_id).await {
                    Ok(()) => tracing::info!(source_id = id, notification_id, "deleted *arr webhook"),
                    Err(e) => tracing::warn!(source_id = id, notification_id, error = %e,
                        "failed to delete *arr webhook; proceeding with local delete"),
                },
                Err(e) => tracing::warn!(source_id = id, error = %e,
                    "failed to construct *arr client; proceeding with local delete"),
            }
        }
    }

    // Always delete the local row, even if the remote call failed.
    sqlx::query("DELETE FROM sources WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
```

NOTE: keep the existing inline `DELETE FROM sources` SQL — the explore showed this is how the current handler does it. If the existing handler used a different return type or error shape, match it.

- [ ] **Step 2: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build.

- [ ] **Step 3: Run wider tests**

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`. The auto-provision test from Task 8 still passes.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/api/sources.rs
git commit -m "feat(api): symmetric teardown of *arr webhook on DELETE /api/sources/:id"
```

---

## Task 10: `api/sources::update` auto-reconcile on URL/key/name change

**Files:**
- Modify: `crates/transcoderr/src/api/sources.rs`

- [ ] **Step 1: Replace the `update` handler body**

In `crates/transcoderr/src/api/sources.rs`, find the existing `update` handler. The current handler reads the row, applies field updates, writes back. After this branch it also needs to detect changes to `base_url` / `api_key` / `name` on auto-provisioned sources and re-call the *arr to delete-old + create-new. Replace its body with:

```rust
pub async fn update(
    State(state): State<crate::http::AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSourceReq>,
) -> Result<StatusCode, StatusCode> {
    let row = match crate::db::sources::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    // Compose the new config (req.config may be None → keep existing).
    let old_cfg: serde_json::Value =
        serde_json::from_str(&row.config_json).unwrap_or_default();
    let mut new_cfg = match req.config {
        Some(ref c) => c.clone(),
        None => old_cfg.clone(),
    };

    let new_name = req.name.clone().unwrap_or_else(|| row.name.clone());
    let arr_kind = crate::arr::Kind::parse(&row.kind);

    let needs_reprovision = arr_kind.is_some()
        && old_cfg.get("arr_notification_id").is_some()
        && (
            old_cfg.get("base_url") != new_cfg.get("base_url")
                || old_cfg.get("api_key") != new_cfg.get("api_key")
                || new_name != row.name
        );

    if needs_reprovision {
        let arr_kind = arr_kind.unwrap();
        let old_id = old_cfg.get("arr_notification_id").and_then(|v| v.as_i64()).unwrap();

        // Best-effort delete of the OLD webhook against the OLD creds.
        if let (Some(old_base), Some(old_key)) = (
            old_cfg.get("base_url").and_then(|v| v.as_str()),
            old_cfg.get("api_key").and_then(|v| v.as_str()),
        ) {
            if let Ok(c) = crate::arr::Client::new(old_base, old_key) {
                if let Err(e) = c.delete_notification(old_id).await {
                    tracing::warn!(source_id = id, old_id, error = %e,
                        "failed to delete old *arr webhook during update; proceeding");
                }
            }
        }

        // Mandatory create of the NEW webhook against the NEW creds.
        let new_base = new_cfg.get("base_url").and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let new_key = new_cfg.get("api_key").and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let webhook_url = format!("{}/webhook/{}", state.public_url, row.kind);
        let client = crate::arr::Client::new(new_base, new_key)
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        let new_n = client
            .create_notification(arr_kind, &new_name, &webhook_url, &row.secret_token)
            .await
            .map_err(|e| {
                tracing::error!(source_id = id, error = %e, "failed to provision new *arr webhook on update");
                StatusCode::BAD_GATEWAY
            })?;

        if let Some(obj) = new_cfg.as_object_mut() {
            obj.insert("arr_notification_id".into(), serde_json::json!(new_n.id));
        }
    }

    // Write the new row. Note: secret_token is NOT updated here even if
    // req.secret_token is Some — for auto-provisioned sources the token
    // is server-managed; for manual sources update via the existing
    // pre-existing path. If req.secret_token is Some AND not "***",
    // honor it for manual sources only.
    let new_secret = match (arr_kind, req.secret_token.as_deref()) {
        (Some(_), _) => row.secret_token.clone(), // auto-provisioned: never change token
        (None, Some(s)) if s != "***" => s.to_string(),
        _ => row.secret_token.clone(),
    };
    let cfg_str = serde_json::to_string(&new_cfg)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    sqlx::query("UPDATE sources SET name = ?, config_json = ?, secret_token = ? WHERE id = ?")
        .bind(&new_name)
        .bind(&cfg_str)
        .bind(&new_secret)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
```

NOTE: this replaces the existing inline SQL. If the existing handler had different field-handling logic (e.g. allowed config to be a partial-merge instead of a full replace), preserve that semantic — the snippet above does a FULL replace of `config` when `req.config` is `Some`. Adjust to whatever the existing behavior was.

- [ ] **Step 2: Build and run wider tests**

Run: `cargo build -p transcoderr 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`.

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/api/sources.rs
git commit -m "feat(api): re-provision *arr webhook on PUT /api/sources/:id when base_url/api_key/name changes"
```

---

## Task 11: Boot reconciler

**Files:**
- Create: `crates/transcoderr/src/arr/reconcile.rs`
- Modify: `crates/transcoderr/src/arr/mod.rs` (re-export `pub mod reconcile;`)
- Modify: `crates/transcoderr/src/main.rs` (spawn after `axum::serve`)

- [ ] **Step 1: Create the reconciler module**

Create `crates/transcoderr/src/arr/reconcile.rs`:

```rust
//! One-shot reconciler that runs at boot. For each auto-provisioned
//! source (kind in radarr/sonarr/lidarr AND `arr_notification_id`
//! present in `config_json`), fetch the corresponding notification
//! from the *arr and verify URL + secret_token still match. If
//! either drifted, DELETE + recreate. Cosmetic drift (event flags,
//! display name) is intentionally tolerated.

use crate::arr;
use crate::db;
use sqlx::SqlitePool;
use std::sync::Arc;

pub fn spawn(pool: SqlitePool, public_url: Arc<String>) {
    tokio::spawn(async move {
        if let Err(e) = run(&pool, &public_url).await {
            tracing::warn!(error = %e, "boot reconciler failed; sources may be in an unexpected state");
        }
    });
}

async fn run(pool: &SqlitePool, public_url: &str) -> anyhow::Result<()> {
    let sources = db::sources::list_all(pool).await?;
    for src in sources {
        let Some(arr_kind) = arr::Kind::parse(&src.kind) else { continue };
        let cfg: serde_json::Value =
            serde_json::from_str(&src.config_json).unwrap_or_default();
        let Some(notification_id) = cfg.get("arr_notification_id").and_then(|v| v.as_i64()) else {
            continue;
        };
        let Some(base_url) = cfg.get("base_url").and_then(|v| v.as_str()) else { continue };
        let Some(api_key) = cfg.get("api_key").and_then(|v| v.as_str()) else { continue };

        if let Err(e) = reconcile_one(pool, &src, arr_kind, base_url, api_key, notification_id, public_url).await {
            tracing::warn!(source_id = src.id, name = %src.name, error = %e, "reconcile failed");
        }
    }
    Ok(())
}

async fn reconcile_one(
    pool: &SqlitePool,
    src: &db::sources::SourceRow,
    arr_kind: arr::Kind,
    base_url: &str,
    api_key: &str,
    notification_id: i64,
    public_url: &str,
) -> anyhow::Result<()> {
    let client = arr::Client::new(base_url, api_key)?;
    let expected_url = format!("{public_url}/webhook/{}", src.kind);

    match client.get_notification(notification_id).await? {
        Some(n) if matches_expected(&n, &expected_url, &src.secret_token) => {
            tracing::info!(source_id = src.id, notification_id, "*arr webhook in sync");
        }
        Some(_) => {
            tracing::warn!(
                source_id = src.id,
                notification_id,
                expected_url = %expected_url,
                "*arr webhook drifted on key fields; recreating"
            );
            client.delete_notification(notification_id).await?;
            let new_n = client
                .create_notification(arr_kind, &src.name, &expected_url, &src.secret_token)
                .await?;
            db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
            tracing::info!(source_id = src.id, old_id = notification_id, new_id = new_n.id, "*arr webhook recreated");
        }
        None => {
            tracing::warn!(source_id = src.id, missing_id = notification_id, "*arr webhook missing; recreating");
            let new_n = client
                .create_notification(arr_kind, &src.name, &expected_url, &src.secret_token)
                .await?;
            db::sources::update_arr_notification_id(pool, src.id, new_n.id).await?;
            tracing::info!(source_id = src.id, new_id = new_n.id, "*arr webhook recreated");
        }
    }
    Ok(())
}

/// Drift detection — only the fields that break delivery. Cosmetic
/// drift (operator-toggled event flags, renamed display name, added
/// tags) is intentionally ignored.
pub(crate) fn matches_expected(
    n: &arr::Notification,
    expected_url: &str,
    expected_secret: &str,
) -> bool {
    let url = n
        .fields
        .iter()
        .find(|f| f.name == "url")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    let password = n
        .fields
        .iter()
        .find(|f| f.name == "password")
        .and_then(|f| f.value.as_str())
        .unwrap_or("");
    url == expected_url && password == expected_secret
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arr::{Field, Notification};

    fn n(url: &str, password: &str) -> Notification {
        Notification {
            id: 1,
            name: "transcoderr-x".into(),
            implementation: "Webhook".into(),
            config_contract: "WebhookSettings".into(),
            fields: vec![
                Field { name: "url".into(), value: serde_json::json!(url) },
                Field { name: "password".into(), value: serde_json::json!(password) },
            ],
            on_grab: false,
            on_download: true,
            on_upgrade: true,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn matches_expected_when_url_and_secret_match() {
        let notif = n("http://t/webhook/radarr", "abc");
        assert!(matches_expected(&notif, "http://t/webhook/radarr", "abc"));
    }

    #[test]
    fn does_not_match_when_url_drifted() {
        let notif = n("http://OLD/webhook/radarr", "abc");
        assert!(!matches_expected(&notif, "http://NEW/webhook/radarr", "abc"));
    }

    #[test]
    fn does_not_match_when_secret_drifted() {
        let notif = n("http://t/webhook/radarr", "OLD");
        assert!(!matches_expected(&notif, "http://t/webhook/radarr", "NEW"));
    }

    #[test]
    fn matches_when_only_event_flags_drifted() {
        // Operator added/removed event flags — we don't care.
        let mut notif = n("http://t/webhook/radarr", "abc");
        notif.on_grab = true;
        notif.name = "operator-renamed".into();
        notif.extra.insert("tags".into(), serde_json::json!([1, 2]));
        assert!(matches_expected(&notif, "http://t/webhook/radarr", "abc"));
    }
}
```

- [ ] **Step 2: Re-export `reconcile` from `arr/mod.rs`**

In `crates/transcoderr/src/arr/mod.rs`, near the top (after the file's doc comment, before the `use` statements), add:

```rust
pub mod reconcile;
```

- [ ] **Step 3: Spawn the reconciler in `serve`**

In `crates/transcoderr/src/main.rs`, find the section just after `axum::serve(listener, app).with_graceful_shutdown(...)` and before `let serve_task = tokio::spawn(async move { serve.await });`. Insert:

```rust
transcoderr::arr::reconcile::spawn(state.pool.clone(), state.public_url.clone());
```

This must run AFTER `state` is constructed (so `state.pool` and `state.public_url` exist) and BEFORE the server task starts (or concurrent with — order doesn't matter as long as it's spawned).

- [ ] **Step 4: Run tests**

Run: `cargo test -p transcoderr --lib reconcile:: 2>&1 | tail -10`
Expected: 4 tests pass.

Run wider: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -25`
Expected: all `ok`.

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add crates/transcoderr/src/arr/reconcile.rs crates/transcoderr/src/arr/mod.rs crates/transcoderr/src/main.rs
git commit -m "feat(arr): boot reconciler verifies *arr webhooks against drift"
```

---

## Task 12: WebUI source-create form (kind-conditional fields)

**Files:**
- Modify: `web/src/pages/sources.tsx`

This is the React side. The existing `sources.tsx` is a single file with inline create form + table. Rework it to render different fields based on the selected `kind`.

- [ ] **Step 1: Update the create form to branch on kind**

In `web/src/pages/sources.tsx`, find the existing create form (single React component with `useState` for `kind`, `name`, `token`, `config`). Replace its render with kind-conditional fields:

For `kind` ∈ {radarr, sonarr, lidarr}:
- `Name` — text, required
- `Base URL` — text, required, placeholder `http://radarr:7878`
- `API key` — password input (`type="password"`), required
- Help text below: "Transcoderr will create the webhook in {Radarr|Sonarr|Lidarr} for you. The connection token is generated automatically."
- **No `secret_token` field**

For `kind` === `generic` or `webhook`:
- `Name`, `Secret token`, `Config` — same as today
- Help text: "Add a webhook in your tool's settings pointing at `{public_url}/webhook/{name}` with this token as the password." (where `{public_url}` is fetched from the server — see Step 2 below)

Submit handler:
- For auto-provision kinds, post `{ kind, name, config: { base_url, api_key }, secret_token: "" }` (server generates the real token).
- For manual kinds, post `{ kind, name, config, secret_token }` as today.

The exact code structure depends on the existing component's shape. The pattern is:

```tsx
const [kind, setKind] = useState<string>("radarr");
const [name, setName] = useState("");
const [baseUrl, setBaseUrl] = useState("");
const [apiKey, setApiKey] = useState("");
const [secretToken, setSecretToken] = useState("");
const [config, setConfig] = useState("{}");

const isAutoProvision = ["radarr", "sonarr", "lidarr"].includes(kind);

// In the form JSX:
{isAutoProvision ? (
  <>
    <input placeholder="Base URL (e.g. http://radarr:7878)" value={baseUrl} onChange={e => setBaseUrl(e.target.value)} required />
    <input type="password" placeholder="API key" value={apiKey} onChange={e => setApiKey(e.target.value)} required />
    <p className="hint">
      Transcoderr will create the webhook in {capitalize(kind)} for you. The connection token is generated automatically.
    </p>
  </>
) : (
  <>
    <input placeholder="Secret token" value={secretToken} onChange={e => setSecretToken(e.target.value)} required />
    <textarea placeholder="Config (JSON)" value={config} onChange={e => setConfig(e.target.value)} />
  </>
)}

// Submit:
const submit = async () => {
  const body = isAutoProvision
    ? { kind, name, config: { base_url: baseUrl, api_key: apiKey }, secret_token: "" }
    : { kind, name, config: JSON.parse(config), secret_token: secretToken };
  await createMutation.mutateAsync(body);
};
```

Adapt to the existing form's layout / styling.

- [ ] **Step 2: Add an "auto" / "manual" badge to the source list**

In the same file, find the `<tr>` rendering for the sources table. For each row, after the kind cell, render a badge:

```tsx
<span className={`badge badge-${isAuto(src) ? "auto" : "manual"}`}>
  {isAuto(src) ? "auto" : "manual"}
</span>
```

Where `isAuto(src)` is:

```tsx
function isAuto(src: Source): boolean {
  return ["radarr", "sonarr", "lidarr"].includes(src.kind)
    && src.config?.arr_notification_id != null;
}
```

Add minimal CSS (e.g. `.badge-auto { background: #d4edda; color: #155724; }` and `.badge-manual { background: #f8d7da; color: #721c24; }`) inline or in the existing stylesheet.

- [ ] **Step 3: Update the edit form (if one exists)**

If the existing `sources.tsx` has an inline edit form, apply the same kind-conditional treatment. For auto-provisioned sources:
- `Base URL` editable (text input pre-filled from current config)
- `API key` shows `***` placeholder; replacing it sends the new value, leaving the placeholder unchanged means "keep existing" (consistent with how the API treats `"***"` for the token)

If the page only supports create + delete (no edit), skip this step and note it as a follow-up.

- [ ] **Step 4: Build the frontend**

Run from the repo root: `cd web && npm run build 2>&1 | tail -10`
Expected: clean TS build (no errors). Warnings about unused imports etc. are fine.

- [ ] **Step 5: Manual smoke test in dev**

Run the frontend dev server: `cd web && npm run dev`
- Navigate to the Sources page.
- Switch the `kind` dropdown between "radarr" and "generic"; confirm the form fields swap.
- The create submit hits the API; if you're running the backend locally with no Radarr to talk to, expect an error from the server — verify the error message renders sensibly in the UI.

- [ ] **Step 6: Commit**

```bash
git branch --show-current   # must print: feature/source-autoprovision
git add web/src/pages/sources.tsx
# include any CSS file you touched:
# git add web/src/...
git commit -m "feat(web): kind-conditional source form + auto/manual badge"
```

---

## Task 13: Verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm branch and clean state**

Run: `git branch --show-current && git status --short`
Expected: `feature/source-autoprovision`, no uncommitted changes.

- [ ] **Step 2: Run the full per-crate test suite**

Run: `cargo test -p transcoderr --locked --lib --tests 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: all `ok` lines. The pre-existing `metrics_endpoint_responds_with_text_format` flake may surface — that's known and not introduced by this branch.

- [ ] **Step 3: Confirm the new tests are present and passing**

Run: `cargo test -p transcoderr --lib arr:: 2>&1 | grep -E "^test " | head -20`
Expected: all `arr::*` and `arr::reconcile::*` tests `ok`. Total ~12 (3 from Task 2, 2 from Task 3, 3 from Task 4, 4 from Task 11).

Run: `cargo test -p transcoderr --lib public_url:: 2>&1 | grep -E "^test " | head`
Expected: 2 lines, all `... ok`.

Run: `cargo test -p transcoderr --test auto_provision 2>&1 | grep -E "^test " | head`
Expected: 1 line, `... ok` (`create_source_radarr_calls_arr_then_persists`).

- [ ] **Step 4: Confirm boot probes log expected output**

Run: `cargo run -q -p transcoderr -- serve --config /nonexistent 2>&1 | grep -E "transcoderr serving|public_url" | head -3`
Expected: a log line including `public_url=...` and `source=Default` (or `source=Env` if `TRANSCODERR_PUBLIC_URL` is set in the runtime). The server will exit on the missing config; we only need to confirm the log prints.

- [ ] **Step 5: WebUI build sanity check**

Run from the repo root: `cd web && npm run build 2>&1 | tail -5`
Expected: clean TypeScript build, no errors. Bundles to `web/dist/`.

- [ ] **Step 6: Branch commit list**

Run: `git log --oneline feature/source-autoprovision ^main`
Expected (in roughly this order, plus the spec + plan commits already on the branch):

```
feat(web): kind-conditional source form + auto/manual badge
feat(arr): boot reconciler verifies *arr webhooks against drift
feat(api): re-provision *arr webhook on PUT /api/sources/:id when base_url/api_key/name changes
feat(api): symmetric teardown of *arr webhook on DELETE /api/sources/:id
feat(api): auto-provision *arr webhook on POST /api/sources
feat(db): list_all / get_by_id / update_arr_notification_id helpers
feat(http): AppState gains public_url; resolved at serve()
feat(public-url): resolve TRANSCODERR_PUBLIC_URL with hostname fallback
feat(arr): Client::list/get/delete_notification with 404 → None
feat(arr): Client::create_notification with per-kind event flags
feat(arr): types + Client skeleton for *arr notification API
build: add hostname (runtime) + wiremock (dev) for arr client
docs(spec): source auto-provisioning for radarr/sonarr/lidarr
```

- [ ] **Step 7: Manual end-to-end on the dev server**

This is the only path that exercises a real Radarr instance. Skip if not reachable; the per-crate tests above cover the unit-testable surface.

If reachable:

1. Build and deploy the binary to the dev server.
2. In Radarr, open Settings → General → API Key and copy it.
3. In transcoderr's WebUI: Sources page → Create → kind=radarr, name=Movies, base_url=`http://radarr:7878` (or whatever your Radarr is reachable at from inside the transcoderr container), api_key=(paste). Submit.
4. Confirm the source appears in the list with an "auto" badge.
5. In Radarr, open Settings → Connect — confirm a "transcoderr-Movies" webhook is present and points to `http://transcoderr:8099/webhook/radarr` (or your `TRANSCODERR_PUBLIC_URL`).
6. Trigger an event (e.g. mark a movie as upgraded). Confirm transcoderr receives the webhook (a new run appears).
7. In Radarr, manually delete the webhook from Settings → Connect.
8. Restart transcoderr. In the boot logs, confirm `*arr webhook missing; recreating`. Re-check Radarr's Settings → Connect — webhook should be back.
9. In transcoderr, edit the source: change `api_key` to a wrong value. Confirm the API returns the *arr's 401 error message; the local source row is unchanged.
10. Fix the api_key, re-update. Confirm Radarr shows a webhook with a fresh notification id.
11. Delete the source from transcoderr. Confirm Radarr's Settings → Connect no longer shows it.

- [ ] **Step 8: (No commit — verification only.)** The branch is ready for review/merge.
