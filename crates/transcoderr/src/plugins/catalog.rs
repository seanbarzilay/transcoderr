use crate::db::plugin_catalogs::{self, CatalogRow};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing;

pub const CATALOG_SCHEMA_VERSION: u32 = 1;
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Top-level shape of a catalog's index.json.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Index {
    pub schema_version: u32,
    #[serde(default)]
    pub catalog_name: Option<String>,
    #[serde(default)]
    pub catalog_url: Option<String>,
    #[serde(default)]
    pub plugins: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexEntry {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub summary: String,
    pub tarball_url: String,
    pub tarball_sha256: String,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub min_transcoderr_version: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub provides_steps: Vec<String>,
    /// Bare executable names the plugin needs on PATH (`["python3"]`,
    /// `["node"]`, etc.). Defaults to empty -- "POSIX shell + coreutils
    /// only", which every supported transcoderr image already has.
    /// Surfaced from manifest.toml by the catalog repo's publish.py.
    #[serde(default)]
    pub runtimes: Vec<String>,
    /// Optional shell command the catalog entry advertises (e.g.
    /// `pip install -r requirements.txt`). Surfaced from manifest.toml
    /// by the catalog repo's publish.py. The Browse tab shows it so the
    /// operator sees what will run on install / boot.
    #[serde(default)]
    pub deps: Option<String>,
}

/// Resolved entry served to the API: an index entry tagged with the
/// catalog it came from, plus a server-computed `missing_runtimes`
/// list so the FE can disable Install before the operator clicks.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub catalog_id: i64,
    pub catalog_name: String,
    #[serde(flatten)]
    pub entry: IndexEntry,
    /// Subset of `entry.runtimes` not on this host's PATH. Empty list
    /// means installable. Populated by the browse handler per-request,
    /// not by the catalog client itself (which has no PATH knowledge).
    #[serde(default)]
    pub missing_runtimes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogFetchError {
    pub catalog_id: i64,
    pub catalog_name: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListAllResult {
    pub entries: Vec<CatalogEntry>,
    pub errors: Vec<CatalogFetchError>,
}

#[derive(Debug)]
struct CacheEntry {
    fetched_at: Option<Instant>,
    result: Result<Vec<IndexEntry>, String>,
}

#[derive(Debug)]
pub struct CatalogClient {
    cache: RwLock<std::collections::HashMap<i64, CacheEntry>>,
}

impl Default for CatalogClient {
    fn default() -> Self {
        Self {
            cache: RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl CatalogClient {
    fn http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client builds")
    }

    pub async fn fetch_index(&self, catalog: &CatalogRow) -> anyhow::Result<Vec<IndexEntry>> {
        let mut req = Self::http_client().get(&catalog.url);
        if let Some(h) = &catalog.auth_header {
            req = req.header("Authorization", h);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("catalog {}: HTTP {}", catalog.name, status);
        }
        let idx: Index = resp.json().await?;
        if idx.schema_version != CATALOG_SCHEMA_VERSION {
            anyhow::bail!(
                "catalog {}: schema_version {} unsupported (expected {})",
                catalog.name,
                idx.schema_version,
                CATALOG_SCHEMA_VERSION
            );
        }
        Ok(idx.plugins)
    }

    /// Fetches all configured catalogs in parallel. Per-catalog failures
    /// are surfaced in `errors` -- the call itself never fails. Cached
    /// for `CACHE_TTL` per catalog id.
    pub async fn list_all(&self, pool: &SqlitePool) -> anyhow::Result<ListAllResult> {
        let catalogs = plugin_catalogs::list(pool).await?;
        let mut entries = Vec::new();
        let mut errors = Vec::new();

        let now = Instant::now();
        let mut to_fetch: Vec<&CatalogRow> = Vec::new();
        {
            let cache = self.cache.read().await;
            for c in &catalogs {
                match cache.get(&c.id) {
                    Some(e)
                        if e.fetched_at
                            .is_some_and(|t| now.duration_since(t) < CACHE_TTL) =>
                    {
                        match &e.result {
                            Ok(plugins) => {
                                for p in plugins {
                                    entries.push(CatalogEntry {
                                        catalog_id: c.id,
                                        catalog_name: c.name.clone(),
                                        entry: p.clone(),
                                        missing_runtimes: Vec::new(),
                                    });
                                }
                            }
                            Err(msg) => errors.push(CatalogFetchError {
                                catalog_id: c.id,
                                catalog_name: c.name.clone(),
                                error: msg.clone(),
                            }),
                        }
                    }
                    _ => to_fetch.push(c),
                }
            }
        }

        // Fetch each uncached/expired catalog. Sequential is fine for
        // the typical "1-3 catalogs" load -- parallelizing adds noise
        // (need to clone CatalogRow into 'static) for negligible win.
        for c in to_fetch {
            let res = self.fetch_index(c).await.map_err(|e| e.to_string());
            let mut cache = self.cache.write().await;
            cache.insert(
                c.id,
                CacheEntry {
                    fetched_at: Some(now),
                    result: res.clone(),
                },
            );
            match res {
                Ok(plugins) => {
                    if let Err(e) = plugin_catalogs::record_fetch_success(pool, c.id).await {
                        tracing::warn!(catalog_id = c.id, error = %e, "failed to persist catalog fetch success");
                    }
                    for p in plugins {
                        entries.push(CatalogEntry {
                            catalog_id: c.id,
                            catalog_name: c.name.clone(),
                            entry: p,
                            missing_runtimes: Vec::new(),
                        });
                    }
                }
                Err(e) => {
                    if let Err(db_err) = plugin_catalogs::record_fetch_error(pool, c.id, &e).await {
                        tracing::warn!(catalog_id = c.id, error = %db_err, "failed to persist catalog fetch error");
                    }
                    errors.push(CatalogFetchError {
                        catalog_id: c.id,
                        catalog_name: c.name.clone(),
                        error: e,
                    });
                }
            }
        }
        Ok(ListAllResult { entries, errors })
    }

    pub async fn invalidate(&self, catalog_id: i64) {
        self.cache.write().await.remove(&catalog_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        sqlx::query("DELETE FROM plugin_catalogs")
            .execute(&pool)
            .await
            .unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn fetch_index_happy_path_returns_plugins() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1,
                "catalog_name": "test",
                "plugins": [{
                    "name": "size-report",
                    "version": "0.1.0",
                    "summary": "size",
                    "tarball_url": "https://example.com/size-report-0.1.0.tar.gz",
                    "tarball_sha256": "abc",
                    "kind": "subprocess",
                    "provides_steps": ["size.report.before", "size.report.after"]
                }]
            })))
            .mount(&server)
            .await;

        let cat = CatalogRow {
            id: 1,
            name: "test".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: None,
            priority: 0,
            last_fetched_at: None,
            last_error: None,
        };
        let plugins = CatalogClient::default().fetch_index(&cat).await.unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "size-report");
        assert_eq!(plugins[0].provides_steps.len(), 2);
    }

    #[tokio::test]
    async fn fetch_index_sends_auth_header_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.json"))
            .and(header("Authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1, "plugins": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cat = CatalogRow {
            id: 1,
            name: "private".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: Some("Bearer secret".into()),
            priority: 0,
            last_fetched_at: None,
            last_error: None,
        };
        CatalogClient::default().fetch_index(&cat).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_index_rejects_wrong_schema_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 99, "plugins": []
            })))
            .mount(&server)
            .await;
        let cat = CatalogRow {
            id: 1,
            name: "future".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: None,
            priority: 0,
            last_fetched_at: None,
            last_error: None,
        };
        let err = CatalogClient::default()
            .fetch_index(&cat)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("schema_version 99"));
    }

    #[tokio::test]
    async fn list_all_aggregates_success_and_failure_per_catalog() {
        let server_ok = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ok.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1,
                "plugins": [{
                    "name": "x", "version": "0.1.0", "summary": "",
                    "tarball_url": "https://e/x.tgz", "tarball_sha256": "h",
                    "kind": "subprocess", "provides_steps": []
                }]
            })))
            .mount(&server_ok)
            .await;
        let server_bad = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bad.json"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server_bad)
            .await;

        let (pool, _dir) = open_pool().await;
        plugin_catalogs::create(
            &pool,
            "ok",
            &format!("{}/ok.json", server_ok.uri()),
            None,
            0,
        )
        .await
        .unwrap();
        plugin_catalogs::create(
            &pool,
            "bad",
            &format!("{}/bad.json", server_bad.uri()),
            None,
            1,
        )
        .await
        .unwrap();

        let res = CatalogClient::default().list_all(&pool).await.unwrap();
        assert_eq!(res.entries.len(), 1);
        assert_eq!(res.entries[0].catalog_name, "ok");
        assert_eq!(res.entries[0].entry.name, "x");
        assert_eq!(res.errors.len(), 1);
        assert_eq!(res.errors[0].catalog_name, "bad");
        assert!(res.errors[0].error.contains("503"));

        // last_error / last_fetched_at recorded.
        let rows = plugin_catalogs::list(&pool).await.unwrap();
        let ok_row = rows.iter().find(|r| r.name == "ok").unwrap();
        let bad_row = rows.iter().find(|r| r.name == "bad").unwrap();
        assert!(ok_row.last_fetched_at.is_some());
        assert!(ok_row.last_error.is_none());
        assert!(bad_row.last_error.as_ref().unwrap().contains("503"));
    }

    #[tokio::test]
    async fn list_all_serves_from_cache_within_ttl() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1, "plugins": []
            })))
            .expect(1) // <-- key: only ONE call expected
            .mount(&server)
            .await;

        let (pool, _dir) = open_pool().await;
        plugin_catalogs::create(&pool, "c", &format!("{}/index.json", server.uri()), None, 0)
            .await
            .unwrap();

        let client = CatalogClient::default();
        client.list_all(&pool).await.unwrap();
        client.list_all(&pool).await.unwrap(); // cache hit -- no second HTTP call
    }
}
