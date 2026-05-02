# Webhook Step Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a builtin `webhook` step that fires an arbitrary templated HTTP request from inside a flow.

**Architecture:** New file `crates/transcoderr/src/steps/webhook.rs` implementing the existing `Step` trait, registered in `steps/builtin.rs`. Config is deserialized from YAML via `serde_json::from_value` of a `WebhookConfig` struct with `deny_unknown_fields`. URL/header values/body are templated through `crate::flow::expr::eval_string_template` (same engine `notify` uses). HTTP via a fresh `reqwest::Client` per step (matches the existing webhook-notifier pattern at `crates/transcoderr/src/notifiers/webhook.rs:18`). Hard-fails on network error or non-2xx; `ignore_errors: true` flips both to warn-and-succeed.

**Tech Stack:** Rust 2021, axum/tokio runtime, reqwest (already in deps with rustls-tls), serde + serde_json, anyhow, async-trait, wiremock for integration tests (already in dev-deps).

**Branch:** All tasks land on `feat/webhook-step` off `main`. Implementer creates the branch before Task 1.

**HTTP client decision:** **fresh `reqwest::Client::new()` per step.** Matches existing pattern at `crates/transcoderr/src/notifiers/webhook.rs:18`. The cost (one TLS handshake's worth of state) is negligible compared to the network call itself. No need to thread a shared client through `register_all`.

---

## File Structure

**New files:**
- `crates/transcoderr/src/steps/webhook.rs` — `WebhookConfig` struct, render/validate helpers, `WebhookStep` Step impl, unit tests
- `crates/transcoderr/tests/step_webhook.rs` — integration tests using wiremock
- `docs/flows/webhook.yaml` — example flow

**Modified files:**
- `crates/transcoderr/src/flow/expr.rs` — add `env.*` binding (Task 1, prerequisite)
- `crates/transcoderr/src/steps/builtin.rs` — register the new step (Task 4)
- `README.md` — one-line mention in flow steps list (Task 6)

---

## Task 1: Add `env.*` templating binding

The webhook step's example uses `{{ env.MY_TOKEN }}` for `Authorization` headers, but the current template engine (`crate::flow::expr`) only binds the `Context` struct's fields (`file`, `probe`, `steps`, `failed`). It does not expose process environment variables. This task adds that binding so the spec's templating contract holds.

**Files:**
- Modify: `crates/transcoderr/src/flow/expr.rs`

- [ ] **Step 1: Read the current `bind_context`**

```bash
sed -n '46,55p' crates/transcoderr/src/flow/expr.rs
```

Expected (existing code):

```rust
fn bind_context(cel: &mut CelCtx, ctx: &Context) {
    let v = serde_json::to_value(ctx).unwrap_or(Value::Null);
    if let Value::Object(map) = v {
        for (k, vv) in map {
            cel.add_variable(k, vv).ok();
        }
    }
}
```

- [ ] **Step 2: Replace `bind_context` to also inject `env`**

Use the Edit tool to replace the body of `bind_context` with:

```rust
fn bind_context(cel: &mut CelCtx, ctx: &Context) {
    let v = serde_json::to_value(ctx).unwrap_or(Value::Null);
    if let Value::Object(map) = v {
        for (k, vv) in map {
            cel.add_variable(k, vv).ok();
        }
    }
    // Bind process environment as `env.<NAME>` so templates like
    // `{{ env.MY_TOKEN }}` resolve. Cheap to build per evaluation; the
    // template engine is already not on a hot path (one call per step).
    let env_map: serde_json::Map<String, Value> = std::env::vars()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    cel.add_variable("env", Value::Object(env_map)).ok();
}
```

- [ ] **Step 3: Add a unit test for env templating**

Append to the existing `#[cfg(test)] mod tests` block in `crates/transcoderr/src/flow/expr.rs`:

```rust
    #[test]
    fn env_var_resolves_in_template() {
        // SAFETY: writes the key for the duration of the test.
        // SAFETY: not parallel-unsafe with the rest of the suite because the
        // key is unique to this test — no other test reads or writes it.
        unsafe { std::env::set_var("TCR_TEST_TEMPLATE_KEY", "hello") };
        let ctx = Context::for_file("/m/Dune.mkv");
        let s = eval_string_template("v={{ env.TCR_TEST_TEMPLATE_KEY }}", &ctx).unwrap();
        assert_eq!(s, "v=hello");
    }
```

- [ ] **Step 4: Run the test**

```bash
cargo test -p transcoderr --lib flow::expr 2>&1 | tail -10
```

Expected: all `flow::expr::tests::*` tests pass, including the new `env_var_resolves_in_template`.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/flow/expr.rs
git commit -m "feat(flow): bind \`env.*\` in template engine"
```

---

## Task 2: `WebhookConfig` struct + serde tests

A typed config struct with `deny_unknown_fields` so a typo in the operator's YAML (e.g. `urls:` for `url:`) fails flow-parse with a clear error rather than silently ignoring the field.

**Files:**
- Create: `crates/transcoderr/src/steps/webhook.rs`

- [ ] **Step 1: Create the file with the struct + parse helper + tests**

Create `crates/transcoderr/src/steps/webhook.rs`:

```rust
//! `webhook` builtin step: fires an arbitrary HTTP request whose URL,
//! header values, and body are templated through the same engine
//! `notify` uses. Hard-fails on network error / non-2xx by default;
//! `ignore_errors: true` flips both to warn-and-succeed.

use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub ignore_errors: bool,
}

fn default_method() -> String { "POST".into() }
fn default_timeout_seconds() -> u64 { 30 }

impl WebhookConfig {
    /// Deserialize the step's `with:` map into a typed config. Returns a
    /// configuration error (not a runtime error) so a misconfigured flow
    /// fails fast.
    pub fn from_with(with: &BTreeMap<String, Value>) -> anyhow::Result<Self> {
        let v = Value::Object(with.clone().into_iter().collect());
        serde_json::from_value(v)
            .map_err(|e| anyhow::anyhow!("webhook: invalid `with`: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(v: Value) -> anyhow::Result<WebhookConfig> {
        let with: BTreeMap<String, Value> = match v {
            Value::Object(m) => m.into_iter().collect(),
            _ => panic!("test bug: pass an object"),
        };
        WebhookConfig::from_with(&with)
    }

    #[test]
    fn defaults_method_post_timeout_30_ignore_false() {
        let c = cfg(json!({"url": "https://example.com"})).unwrap();
        assert_eq!(c.url, "https://example.com");
        assert_eq!(c.method, "POST");
        assert_eq!(c.timeout_seconds, 30);
        assert!(!c.ignore_errors);
        assert!(c.headers.is_empty());
        assert!(c.body.is_none());
    }

    #[test]
    fn parses_full_config() {
        let c = cfg(json!({
            "url": "https://x.test",
            "method": "PUT",
            "headers": {"X-A": "1", "X-B": "2"},
            "body": "{}",
            "timeout_seconds": 5,
            "ignore_errors": true
        })).unwrap();
        assert_eq!(c.method, "PUT");
        assert_eq!(c.headers.get("X-A").unwrap(), "1");
        assert_eq!(c.body.as_deref(), Some("{}"));
        assert_eq!(c.timeout_seconds, 5);
        assert!(c.ignore_errors);
    }

    #[test]
    fn missing_url_is_error() {
        let err = cfg(json!({})).unwrap_err();
        assert!(err.to_string().contains("missing field `url`"),
                "got: {}", err);
    }

    #[test]
    fn unknown_field_is_error() {
        let err = cfg(json!({"url": "https://x", "urls": "typo"})).unwrap_err();
        assert!(err.to_string().contains("unknown field"),
                "got: {}", err);
    }
}
```

- [ ] **Step 2: Wire the new module into `mod.rs`**

```bash
sed -n '1,20p' crates/transcoderr/src/steps/mod.rs
```

Find the block of `pub mod xxx;` declarations. Add `pub mod webhook;` alphabetically (between `verify_playable;` and the next entry, or wherever it fits the existing order).

If the existing list is, e.g.:

```rust
pub mod verify_playable;
```

After the edit it reads:

```rust
pub mod verify_playable;
pub mod webhook;
```

- [ ] **Step 3: Run the new tests**

```bash
cargo test -p transcoderr --lib steps::webhook 2>&1 | tail -10
```

Expected: 4 tests pass (`defaults_method_post_timeout_30_ignore_false`, `parses_full_config`, `missing_url_is_error`, `unknown_field_is_error`).

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/steps/webhook.rs crates/transcoderr/src/steps/mod.rs
git commit -m "feat(steps): WebhookConfig with deny_unknown_fields + parse tests"
```

---

## Task 3: Render + validate helpers

Render the templated fields and validate the post-render result. Validation rules:
- URL parses as a valid URL
- Scheme is `http` or `https`
- Method is one of `GET`, `POST`, `PUT`, `PATCH`, `DELETE` (uppercased)
- Body is `None` for `GET`/`DELETE`
- Timeout is clamped to `[1, 300]`

**Files:**
- Modify: `crates/transcoderr/src/steps/webhook.rs`

- [ ] **Step 1: Add a `RenderedRequest` struct + `WebhookConfig::render` method**

In `crates/transcoderr/src/steps/webhook.rs`, after the existing `impl WebhookConfig`, add:

```rust
/// All-strings request shape after templating + validation. Ready to
/// hand to reqwest.
#[derive(Debug, PartialEq, Eq)]
pub struct RenderedRequest {
    pub url: String,
    pub method: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<String>,
    pub timeout_seconds: u64,
    pub ignore_errors: bool,
}

impl WebhookConfig {
    /// Render templates and validate. Returns a configuration error on
    /// any post-render rule violation.
    pub fn render(&self, ctx: &crate::flow::Context) -> anyhow::Result<RenderedRequest> {
        use crate::flow::expr::eval_string_template;

        let url = eval_string_template(&self.url, ctx)?;
        let parsed = url::Url::parse(&url)
            .map_err(|e| anyhow::anyhow!("webhook: url {url:?} did not parse: {e}"))?;
        match parsed.scheme() {
            "http" | "https" => {}
            other => anyhow::bail!("webhook: scheme {other:?} not allowed (use http or https)"),
        }

        let method = self.method.to_uppercase();
        match method.as_str() {
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" => {}
            other => anyhow::bail!("webhook: method {other:?} not allowed"),
        }

        let mut headers = BTreeMap::new();
        for (k, v) in &self.headers {
            let rendered = eval_string_template(v, ctx)?;
            headers.insert(k.clone(), rendered);
        }

        let body = match &self.body {
            Some(t) => Some(eval_string_template(t, ctx)?),
            None => None,
        };
        if body.is_some() && (method == "GET" || method == "DELETE") {
            anyhow::bail!("webhook: body not allowed for {method}");
        }

        let timeout_seconds = self.timeout_seconds.clamp(1, 300);

        Ok(RenderedRequest {
            url, method, headers, body, timeout_seconds,
            ignore_errors: self.ignore_errors,
        })
    }
}
```

- [ ] **Step 2: Confirm `url` crate is available**

```bash
grep -E "^url\s*=" crates/transcoderr/Cargo.toml /Users/seanbarzilay/projects/private/transcoderr/Cargo.toml
```

Expected: at least one match (the `url` crate is used elsewhere in the workspace). If neither file has it, add `url = "2"` to `crates/transcoderr/Cargo.toml`'s `[dependencies]` section before continuing.

- [ ] **Step 3: Append render tests**

In the existing `#[cfg(test)] mod tests` block in `webhook.rs`, append after the existing tests:

```rust
    use crate::flow::Context;

    fn render(v: Value, ctx: &Context) -> anyhow::Result<RenderedRequest> {
        cfg(v).unwrap().render(ctx)
    }

    #[test]
    fn templates_url_headers_body() {
        // SAFETY: unique key, not used by other tests.
        unsafe { std::env::set_var("TCR_WEBHOOK_TEST_TOKEN", "abc") };
        let ctx = Context::for_file("/movies/Foo.mkv");
        let r = render(json!({
            "url": "https://api.test/{{ file.path }}",
            "headers": {
                "Authorization": "Bearer {{ env.TCR_WEBHOOK_TEST_TOKEN }}",
                "X-Path": "{{ file.path }}"
            },
            "body": "p={{ file.path }}"
        }), &ctx).unwrap();
        assert_eq!(r.url, "https://api.test//movies/Foo.mkv");
        assert_eq!(r.headers.get("Authorization").unwrap(), "Bearer abc");
        assert_eq!(r.headers.get("X-Path").unwrap(), "/movies/Foo.mkv");
        assert_eq!(r.body.as_deref(), Some("p=/movies/Foo.mkv"));
        assert_eq!(r.method, "POST");
    }

    #[test]
    fn body_forbidden_for_get() {
        let ctx = Context::for_file("/x");
        let err = render(json!({
            "url": "https://x.test", "method": "GET", "body": "no"
        }), &ctx).unwrap_err();
        assert!(err.to_string().contains("body not allowed"), "got: {}", err);
    }

    #[test]
    fn body_forbidden_for_delete() {
        let ctx = Context::for_file("/x");
        let err = render(json!({
            "url": "https://x.test", "method": "DELETE", "body": "no"
        }), &ctx).unwrap_err();
        assert!(err.to_string().contains("body not allowed"), "got: {}", err);
    }

    #[test]
    fn rejects_non_http_scheme() {
        let ctx = Context::for_file("/x");
        let err = render(json!({"url": "ftp://x.test"}), &ctx).unwrap_err();
        assert!(err.to_string().contains("scheme"), "got: {}", err);
    }

    #[test]
    fn rejects_unparseable_url() {
        let ctx = Context::for_file("/x");
        let err = render(json!({"url": "not a url"}), &ctx).unwrap_err();
        assert!(err.to_string().contains("did not parse"), "got: {}", err);
    }

    #[test]
    fn rejects_unknown_method() {
        let ctx = Context::for_file("/x");
        let err = render(json!({"url": "https://x.test", "method": "OPTIONS"}), &ctx).unwrap_err();
        assert!(err.to_string().contains("not allowed"), "got: {}", err);
    }

    #[test]
    fn lowercase_method_normalized_to_upper() {
        let ctx = Context::for_file("/x");
        let r = render(json!({"url": "https://x.test", "method": "put"}), &ctx).unwrap();
        assert_eq!(r.method, "PUT");
    }

    #[test]
    fn clamps_timeout() {
        let ctx = Context::for_file("/x");
        let r = render(json!({"url": "https://x.test", "timeout_seconds": 9999}), &ctx).unwrap();
        assert_eq!(r.timeout_seconds, 300);
        let r = render(json!({"url": "https://x.test", "timeout_seconds": 0}), &ctx).unwrap();
        assert_eq!(r.timeout_seconds, 1);
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p transcoderr --lib steps::webhook 2>&1 | tail -15
```

Expected: 12 tests pass total (4 from Task 2 + 8 new render tests).

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/steps/webhook.rs crates/transcoderr/Cargo.toml
git commit -m "feat(steps): webhook render + validate (templating, scheme, method, body rules, timeout clamp)"
```

(Cargo.toml only changed if Step 2 had to add the `url` crate.)

---

## Task 4: `WebhookStep` impl + register in builtin

Implement the `Step` trait. Send the rendered request, capture status + first 1024 bytes of response body, branch on `ignore_errors`.

**Files:**
- Modify: `crates/transcoderr/src/steps/webhook.rs`
- Modify: `crates/transcoderr/src/steps/builtin.rs`

- [ ] **Step 1: Append the Step impl to `webhook.rs`**

After the `RenderedRequest` and `impl WebhookConfig::render` blocks, before the `#[cfg(test)] mod tests`:

```rust
const ERROR_BODY_TRUNCATE_BYTES: usize = 1024;

pub struct WebhookStep;

#[async_trait::async_trait]
impl super::Step for WebhookStep {
    fn name(&self) -> &'static str { "webhook" }

    async fn execute(
        &self,
        with: &BTreeMap<String, Value>,
        ctx: &mut crate::flow::Context,
        on_progress: &mut (dyn FnMut(super::StepProgress) + Send),
    ) -> anyhow::Result<()> {
        let cfg = WebhookConfig::from_with(with)?;
        let req = cfg.render(ctx)?;

        on_progress(super::StepProgress::Log(
            format!("webhook: {} {}", req.method, req.url),
        ));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(req.timeout_seconds))
            .build()?;

        let mut builder = client.request(
            reqwest::Method::from_bytes(req.method.as_bytes())
                .map_err(|e| anyhow::anyhow!("webhook: bad method {}: {e}", req.method))?,
            &req.url,
        );
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = &req.body {
            builder = builder.body(body.clone());
        }

        let result = builder.send().await;
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("webhook: {} {}: {e}", req.method, req.url);
                if req.ignore_errors {
                    tracing::warn!(error = %e, url = %req.url, "webhook ignored network error");
                    on_progress(super::StepProgress::Log(format!("{msg} (ignored)")));
                    return Ok(());
                }
                return Err(anyhow::anyhow!(msg));
            }
        };

        let status = resp.status();
        if status.is_success() {
            on_progress(super::StepProgress::Log(format!("webhook: {status}")));
            return Ok(());
        }

        // Non-2xx: capture (and truncate) the body for diagnosis.
        let body = resp.text().await.unwrap_or_default();
        let truncated: String = body.chars().take(ERROR_BODY_TRUNCATE_BYTES).collect();
        let msg = format!("webhook: {} {} -> {status}: {truncated}", req.method, req.url);

        if req.ignore_errors {
            tracing::warn!(status = %status, url = %req.url, body = %truncated, "webhook ignored non-2xx");
            on_progress(super::StepProgress::Log(format!("{msg} (ignored)")));
            return Ok(());
        }

        Err(anyhow::anyhow!(msg))
    }
}
```

- [ ] **Step 2: Register the step in `builtin.rs`**

In `crates/transcoderr/src/steps/builtin.rs`, add to the `use crate::steps::{ ... }` import block:

```rust
    webhook::WebhookStep,
```

Add to `register_all`'s body, alongside the other simple registrations (e.g. between `notify` and the closing brace):

```rust
    map.insert("webhook".into(), Arc::new(WebhookStep));
```

- [ ] **Step 3: Build to confirm no compile errors**

```bash
cargo build -p transcoderr 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/steps/webhook.rs crates/transcoderr/src/steps/builtin.rs
git commit -m "feat(steps): WebhookStep — sends request, branches on ignore_errors"
```

---

## Task 5: Integration tests with wiremock

End-to-end tests using a mock HTTP server. Each test exercises one path: success, non-2xx, non-2xx with ignore, network error, network error with ignore, and a round-trip that asserts the rendered request matches the YAML config.

**Files:**
- Create: `crates/transcoderr/tests/step_webhook.rs`

- [ ] **Step 1: Create the test file**

Create `crates/transcoderr/tests/step_webhook.rs`:

```rust
//! Integration tests for the `webhook` builtin step. Drives the step's
//! `execute` directly against a wiremock server; bypasses the flow
//! engine because the templating + HTTP path is what we actually want
//! to verify.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use transcoderr::flow::Context;
use transcoderr::steps::webhook::WebhookStep;
use transcoderr::steps::{Step, StepProgress};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn with_map(v: Value) -> BTreeMap<String, Value> {
    match v {
        Value::Object(m) => m.into_iter().collect(),
        _ => panic!("test bug: pass an object"),
    }
}

async fn run(with: Value, ctx: &mut Context) -> anyhow::Result<()> {
    let step = WebhookStep;
    let mut cb = |_: StepProgress| {};
    step.execute(&with_map(with), ctx, &mut cb).await
}

#[tokio::test]
async fn success_2xx_step_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/notify"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server).await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(json!({"url": format!("{}/notify", server.uri())}), &mut ctx)
        .await.unwrap();
}

#[tokio::test]
async fn non_2xx_step_fails_with_truncated_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/notify"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal explosion"))
        .mount(&server).await;

    let mut ctx = Context::for_file("/m/x.mkv");
    let err = run(json!({"url": format!("{}/notify", server.uri())}), &mut ctx)
        .await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "must include status: {msg}");
    assert!(msg.contains("internal explosion"), "must include body: {msg}");
}

#[tokio::test]
async fn non_2xx_with_ignore_errors_step_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).and(path("/notify"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server).await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(json!({
        "url": format!("{}/notify", server.uri()),
        "ignore_errors": true,
    }), &mut ctx).await.unwrap();
}

#[tokio::test]
async fn network_error_step_fails() {
    // Port 1 isn't listening; reqwest will get a connection refused.
    let mut ctx = Context::for_file("/m/x.mkv");
    let err = run(json!({"url": "http://127.0.0.1:1/x"}), &mut ctx)
        .await.unwrap_err();
    assert!(err.to_string().contains("webhook:"), "got: {}", err);
}

#[tokio::test]
async fn network_error_with_ignore_errors_ok() {
    let mut ctx = Context::for_file("/m/x.mkv");
    run(json!({
        "url": "http://127.0.0.1:1/x",
        "ignore_errors": true,
    }), &mut ctx).await.unwrap();
}

#[tokio::test]
async fn templated_url_headers_body_round_trip() {
    let server = MockServer::start().await;
    // Wiremock asserts: must POST /notify, with X-Source header set to
    // "transcoderr", and body containing the templated file.path.
    Mock::given(method("POST"))
        .and(path("/notify"))
        .and(header("X-Source", "transcoderr"))
        .and(body_string_contains("/movies/Foo.mkv"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server).await;

    let mut ctx = Context::for_file("/movies/Foo.mkv");
    run(json!({
        "url": format!("{}/notify", server.uri()),
        "headers": {"X-Source": "transcoderr"},
        "body": "{{ file.path }}",
    }), &mut ctx).await.unwrap();
    // Mock's `expect(1)` is verified at drop.
}

#[tokio::test]
async fn body_omitted_for_get_when_unset() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server).await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(json!({
        "url": format!("{}/ping", server.uri()),
        "method": "GET",
    }), &mut ctx).await.unwrap();
}
```

- [ ] **Step 2: Confirm the `webhook` module is exported from the lib**

```bash
grep -n "pub mod webhook\|pub use.*webhook" crates/transcoderr/src/steps/mod.rs
```

Expected: a `pub mod webhook;` line. If the module isn't `pub`, change it (added in Task 2 Step 2 — should already be `pub mod`).

Also confirm `transcoderr::steps::webhook::WebhookStep` is reachable from integration tests (integration tests can only see `pub` items). The test's import is `use transcoderr::steps::webhook::WebhookStep;` so `WebhookStep`'s visibility must be `pub` (it is, from Task 4 Step 1).

- [ ] **Step 3: Run the integration tests**

```bash
cargo test -p transcoderr --test step_webhook 2>&1 | tail -15
```

Expected: 7 tests pass.

- [ ] **Step 4: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/step_webhook.rs
git commit -m "test(steps): wiremock integration tests for webhook step"
```

---

## Task 6: Example flow + README mention

**Files:**
- Create: `docs/flows/webhook.yaml`
- Modify: `README.md`

- [ ] **Step 1: Write the example flow**

Create `docs/flows/webhook.yaml`:

```yaml
# Example: fire a custom webhook on completion (and again on failure).
#
# Templated values:
#   {{ file.path }}                 — the input file path
#   {{ file.size_bytes }}           — input size in bytes (may be null pre-probe)
#   {{ env.MY_API_TOKEN }}          — process environment variable
#   {{ failed.id }} / {{ failed.error }}  — only inside on_failure: chains

name: webhook-demo
triggers:
  - radarr: [downloaded]

steps:
  - use: probe
  - use: plan.init
  - use: plan.video.encode
    with: { codec: x265, crf: 19, preset: fast, hw: { prefer: [nvenc, qsv, vaapi, videotoolbox], fallback: cpu } }
  - use: plan.execute
  - use: verify.playable
  - use: output
    with: { mode: replace }

  # Fire-and-check primary webhook. Hard-fails the run on non-2xx so
  # downstream systems aren't told about a transcode that "worked"
  # when the receiver couldn't accept it.
  - use: webhook
    with:
      url: https://hooks.example.com/transcoderr/done
      method: POST
      headers:
        Content-Type: application/json
        Authorization: "Bearer {{ env.MY_API_TOKEN }}"
      body: |
        {"file": "{{ file.path }}", "size_bytes": {{ file.size_bytes }}}

  # Best-effort metric ping. Don't fail the run if the metrics endpoint
  # is flaky; just log a warning.
  - use: webhook
    with:
      url: https://metrics.example.com/v1/event
      method: POST
      body: '{"event": "transcode_done"}'
      ignore_errors: true

on_failure:
  - use: webhook
    with:
      url: https://hooks.example.com/transcoderr/failed
      headers:
        Authorization: "Bearer {{ env.MY_API_TOKEN }}"
      body: |
        {"file": "{{ file.path }}", "step": "{{ failed.id }}", "error": "{{ failed.error }}"}
```

- [ ] **Step 2: Add a one-line README mention**

In `README.md`, find the example flow code block (around the `## Example flow` heading) or the first list of step names you can find. Add a single bullet to the closest list of builtin step types, or a short paragraph near the docs/flows links pointing readers to the new example.

Concrete edit: locate the line:

```
[`docs/flows/hevc-normalize.yaml`](docs/flows/hevc-normalize.yaml) re-encodes
```

Immediately before that paragraph (or after, depending on layout), insert a short mention:

```markdown
A second example, [`docs/flows/webhook.yaml`](docs/flows/webhook.yaml),
shows the `webhook` step — fire an arbitrary templated HTTP request
(URL, method, headers, body) inline from a flow.
```

If the exact anchor text differs, find the existing line that mentions `hevc-normalize.yaml` and place the new sentence next to it. Don't disturb other content.

- [ ] **Step 3: Verify the example parses**

```bash
cargo run -p transcoderr -- parse-flow --file docs/flows/webhook.yaml 2>&1 | tail -5
```

If `parse-flow` isn't a known subcommand (the CLI may not expose flow parsing), skip this step and rely on the next one.

```bash
cargo build -p transcoderr 2>&1 | tail -3
```

Expected: clean build.

- [ ] **Step 4: Run the full test suite once**

```bash
cargo test -p transcoderr 2>&1 | tail -5
```

Expected: all tests pass (last line shows the largest binary's `test result: ok.`).

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/webhook-step" || { echo "WRONG BRANCH"; exit 1; }
git add docs/flows/webhook.yaml README.md
git commit -m "docs: example webhook-step flow + README mention"
```

---

## Self-Review Notes

This plan covers every section in the spec:
- **Goal + builtin choice** → Task 4 (registers `webhook` in `builtin::register_all`)
- **YAML schema** → Task 2 (`WebhookConfig`) + Task 3 (`render`)
- **Templating** (file/flow/steps/env/failed) → Task 1 (env binding) + Task 3 (render via `eval_string_template`)
- **Failure behavior** (hard-fail by default + `ignore_errors`) → Task 4
- **No response data exposed** → Task 4 (no writes to `ctx.steps`)
- **Body forbidden for GET/DELETE / scheme allowlist / method allowlist / timeout clamp** → Task 3
- **`deny_unknown_fields`** → Task 2
- **Response body truncation to 1024 bytes** → Task 4 (constant `ERROR_BODY_TRUNCATE_BYTES`)
- **HTTP client decision** → Task 4 (per-step `reqwest::Client::new()`); rationale at top of plan
- **Tests** (unit + wiremock integration) → Tasks 2, 3, 5
- **Example flow + README** → Task 6

Each task is one coherent commit on `feat/webhook-step`. No task depends on a later task's symbols.
