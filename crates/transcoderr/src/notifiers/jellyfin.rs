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
///
/// `path_mappings` rewrites `ctx.file.path` to whatever path Jellyfin
/// has the same file mounted at. Required when transcoderr and
/// Jellyfin run in different containers with different bind mounts —
/// otherwise Jellyfin silently no-ops the update because no library
/// item matches the path. First prefix match wins.
pub struct Jellyfin {
    url: String,
    api_key: String,
    path_mappings: Vec<PathMapping>,
}

#[derive(Debug, Clone)]
struct PathMapping {
    from: String,
    to: String,
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
        let path_mappings = parse_path_mappings(&cfg["path_mappings"])?;
        Ok(Self {
            url,
            api_key,
            path_mappings,
        })
    }
}

fn parse_path_mappings(v: &Value) -> anyhow::Result<Vec<PathMapping>> {
    let Some(arr) = v.as_array() else {
        return Ok(vec![]);
    };
    arr.iter()
        .map(|m| {
            let from = m["from"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("jellyfin: path_mappings entry missing 'from'"))?
                .to_string();
            let to = m["to"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("jellyfin: path_mappings entry missing 'to'"))?
                .to_string();
            Ok(PathMapping { from, to })
        })
        .collect()
}

fn rewrite_path(path: &str, mappings: &[PathMapping]) -> String {
    for m in mappings {
        if let Some(rest) = path.strip_prefix(&m.from) {
            return format!("{}{}", m.to.trim_end_matches('/'), rest);
        }
    }
    path.to_string()
}

#[async_trait]
impl Notifier for Jellyfin {
    async fn send(&self, _message: &str, extra: &Value) -> anyhow::Result<()> {
        // The notify step passes `{"file": ctx.file.path}` -- where
        // ctx.file.path is a string, not a nested object.
        let path = extra.get("file").and_then(|f| f.as_str());
        let client = reqwest::Client::new();

        let resp = match path {
            Some(p) => {
                let mapped = rewrite_path(p, &self.path_mappings);
                let body = json!({
                    "Updates": [{ "Path": mapped, "UpdateType": "Modified" }]
                });
                client
                    .post(format!("{}/Library/Media/Updated", self.url))
                    .header("X-Emby-Token", &self.api_key)
                    .json(&body)
                    .send()
                    .await?
            }
            None => {
                // Test button path (extra is Null). Validate URL +
                // token without triggering a real scan.
                client
                    .get(format!("{}/System/Info", self.url))
                    .header("X-Emby-Token", &self.api_key)
                    .send()
                    .await?
            }
        };

        let status = resp.status();
        if !status.is_success() {
            // Surface Jellyfin's own error body in the run timeline so
            // the operator sees *why* the rescan failed (path not in
            // any library, auth scope wrong, etc.) instead of just a
            // bare status code.
            let body = resp.text().await.unwrap_or_default();
            let trimmed = body.trim();
            if trimmed.is_empty() {
                anyhow::bail!("jellyfin: {}", status);
            } else {
                anyhow::bail!("jellyfin: {} - {}", status, trimmed);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn rewrite_path_swaps_prefix_when_match() {
        let mappings = vec![PathMapping {
            from: "/mnt/movies".into(),
            to: "/media/movies".into(),
        }];
        assert_eq!(
            rewrite_path("/mnt/movies/Foo (2024)/Foo.mkv", &mappings),
            "/media/movies/Foo (2024)/Foo.mkv"
        );
    }

    #[test]
    fn rewrite_path_uses_first_matching_prefix() {
        let mappings = vec![
            PathMapping {
                from: "/mnt/tv".into(),
                to: "/media/tv".into(),
            },
            PathMapping {
                from: "/mnt/movies".into(),
                to: "/media/movies".into(),
            },
        ];
        assert_eq!(
            rewrite_path("/mnt/tv/Show/S01E01.mkv", &mappings),
            "/media/tv/Show/S01E01.mkv"
        );
    }

    #[test]
    fn rewrite_path_unchanged_when_no_prefix_matches() {
        let mappings = vec![PathMapping {
            from: "/mnt/movies".into(),
            to: "/media/movies".into(),
        }];
        assert_eq!(
            rewrite_path("/srv/elsewhere/x.mkv", &mappings),
            "/srv/elsewhere/x.mkv"
        );
    }

    #[test]
    fn rewrite_path_handles_trailing_slash_on_to() {
        let mappings = vec![PathMapping {
            from: "/mnt/movies".into(),
            to: "/media/movies/".into(),
        }];
        assert_eq!(
            rewrite_path("/mnt/movies/Foo.mkv", &mappings),
            "/media/movies/Foo.mkv"
        );
    }

    #[tokio::test]
    async fn send_with_file_posts_library_media_updated() {
        // Extra shape mirrors what the notify step actually passes:
        // `{"file": "<path string>"}` -- not a nested object. The earlier
        // test used `{"file": {"path": "..."}}` which never occurs in
        // production and masked the bug that the notifier wasn't seeing
        // the path at all.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Library/Media/Updated"))
            .and(header("X-Emby-Token", "test-key"))
            .and(body_partial_json(json!({
                "Updates": [{ "Path": "/mnt/movies/Foo.mkv", "UpdateType": "Modified" }]
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "test-key",
        }))
        .unwrap();

        jf.send("ignored", &json!({"file": "/mnt/movies/Foo.mkv"}))
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
    async fn send_applies_path_mapping_before_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Library/Media/Updated"))
            .and(body_partial_json(json!({
                "Updates": [{ "Path": "/media/movies/Foo.mkv", "UpdateType": "Modified" }]
            })))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "k",
            "path_mappings": [
                {"from": "/mnt/movies", "to": "/media/movies"}
            ],
        }))
        .unwrap();

        jf.send("ignored", &json!({"file": "/mnt/movies/Foo.mkv"}))
            .await
            .unwrap();
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

        let err = jf.send("x", &json!({"file": "/x.mkv"})).await.unwrap_err();
        assert!(err.to_string().contains("401"));
    }

    #[tokio::test]
    async fn error_body_is_included_in_run_failure() {
        // 4xx with a body -- the operator should see Jellyfin's reason
        // ("Path is required.", "Access denied.", etc.) in the run
        // timeline, not just a bare status code.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Library/Media/Updated"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_string("Path /mnt/movies/Foo.mkv is not in any library."),
            )
            .mount(&server)
            .await;

        let jf = Jellyfin::new(&json!({
            "url": server.uri(),
            "api_key": "k",
        }))
        .unwrap();

        let err = jf
            .send("x", &json!({"file": "/mnt/movies/Foo.mkv"}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("400"), "status missing: {msg}");
        assert!(msg.contains("not in any library"), "body missing: {msg}");
    }
}
