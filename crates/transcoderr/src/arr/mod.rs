//! Typed client for Radarr / Sonarr / Lidarr's `/api/v3/notification`
//! webhook-management endpoint. All three are servarr forks and share
//! the same JSON shape. `Kind` discriminates which event flags to
//! enable on create.

pub mod reconcile;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
}
