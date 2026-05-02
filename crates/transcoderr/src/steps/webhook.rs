//! `webhook` builtin step: fires an arbitrary HTTP request whose URL,
//! header values, and body are templated through the same engine
//! `notify` uses. Hard-fails on network error / non-2xx by default;
//! `ignore_errors: true` flips both to warn-and-succeed.

use async_trait::async_trait;
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

const ERROR_BODY_TRUNCATE_BYTES: usize = 1024;

pub struct WebhookStep;

#[async_trait]
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
}
