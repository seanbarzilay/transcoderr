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
