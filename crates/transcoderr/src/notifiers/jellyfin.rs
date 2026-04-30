use super::Notifier;
use async_trait::async_trait;
use serde_json::{json, Value};

/// Jellyfin "rescan this file" notifier. POSTs to
/// `/Library/Media/Updated` so the server picks up the freshly
/// transcoded file without a full library scan — same endpoint
/// Sonarr/Radarr's "Connect → Jellyfin" integration uses.
///
/// Read `extra.file.path` (set by the `notify` step from
/// `ctx.file.path`). If missing — the Test button passes
/// `extra: Null` — fall back to a `/System/Info` probe so the test
/// validates the URL + api key without triggering a real scan.
pub struct Jellyfin {
    url: String,
    api_key: String,
}

impl Jellyfin {
    pub fn new(cfg: &Value) -> anyhow::Result<Self> {
        let url = cfg["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("jellyfin: missing url"))?
            .trim_end_matches('/')
            .to_string();
        let api_key = cfg["api_key"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("jellyfin: missing api_key"))?
            .to_string();
        Ok(Self { url, api_key })
    }
}

#[async_trait]
impl Notifier for Jellyfin {
    async fn send(&self, _message: &str, extra: &Value) -> anyhow::Result<()> {
        let path = extra
            .get("file")
            .and_then(|f| f.get("path"))
            .and_then(|p| p.as_str());
        let client = reqwest::Client::new();

        let resp = match path {
            Some(p) => {
                let body = json!({
                    "Updates": [{ "Path": p, "UpdateType": "Modified" }]
                });
                client
                    .post(format!("{}/Library/Media/Updated", self.url))
                    .header("X-Emby-Token", &self.api_key)
                    .json(&body)
                    .send()
                    .await?
            }
            None => {
                client
                    .get(format!("{}/System/Info", self.url))
                    .header("X-Emby-Token", &self.api_key)
                    .send()
                    .await?
            }
        };

        if !resp.status().is_success() {
            anyhow::bail!("jellyfin: {}", resp.status());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn send_with_file_posts_library_media_updated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Library/Media/Updated"))
            .and(header("X-Emby-Token", "test-key"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "test-key",
        }))
        .unwrap();

        jf.send("ignored", &json!({"file": {"path": "/mnt/movies/Foo.mkv"}}))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn send_without_file_probes_system_info() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/System/Info"))
            .and(header("X-Emby-Token", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Version": "10.9"})))
            .expect(1)
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "test-key",
        }))
        .unwrap();

        jf.send("test", &Value::Null).await.unwrap();
    }

    #[tokio::test]
    async fn non_2xx_response_is_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Library/Media/Updated"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "wrong",
        }))
        .unwrap();

        let err = jf
            .send("x", &json!({"file": {"path": "/x.mkv"}}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("401"));
    }
}
