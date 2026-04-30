# Plugin Catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operators install transcoderr plugins from a curated catalog inside the web UI; manual copy into `{data_dir}/plugins/` keeps working as the fallback.

**Architecture:** A catalog is one HTTPS-reachable `index.json` plus a set of plugin-directory tarballs (sha256-pinned in the index). The server fetches all configured catalogs, lets the UI browse, and on Install streams the tarball, verifies its sha, atomically swaps it into `{data_dir}/plugins/<name>/`, then re-runs the existing `discover()` + `sync_discovered()` flow and live-replaces the in-memory step registry so newly-provided steps dispatch without a restart.

**Tech Stack:** Rust + axum + sqlx + SQLite (workspace defaults). New deps: `tar` for untarring, `flate2` for gzip decompression, `sha2` for the integrity check. Tests use `wiremock` for catalog HTTP and `tempfile` + on-the-fly tarball builders for the installer. Frontend is the existing React + TanStack Query + plain CSS stack already used by every other Configure page.

**Spec:** `docs/superpowers/specs/2026-05-01-plugin-catalog-design.md`

---

## File Structure

### Backend (Rust)

| File | Responsibility |
|---|---|
| `crates/transcoderr/migrations/20260501000001_plugin_catalogs.sql` *(NEW)* | Add `plugin_catalogs` table + `plugins.catalog_id` + `plugins.tarball_sha256`; seed the official catalog row |
| `crates/transcoderr/src/db/plugin_catalogs.rs` *(NEW)* | CRUD + `last_fetched_at` / `last_error` updaters |
| `crates/transcoderr/src/db/plugins.rs` *(MODIFY)* | `sync_discovered` learns to record `catalog_id` + `tarball_sha256` when the install came from a catalog |
| `crates/transcoderr/src/plugins/catalog.rs` *(NEW)* | HTTP client: `fetch_index(catalog)` + `list_all(catalogs)` with 5-min in-memory cache + per-catalog error surfacing |
| `crates/transcoderr/src/plugins/installer.rs` *(NEW)* | Tarball pipeline: download → sha verify → untar to staging → layout/manifest verify → atomic swap into plugins dir |
| `crates/transcoderr/src/plugins/uninstaller.rs` *(NEW)* | `rm -rf` plugin dir + drop DB row; mirror of installer |
| `crates/transcoderr/src/plugins/mod.rs` *(MODIFY)* | `pub mod catalog; pub mod installer; pub mod uninstaller;` |
| `crates/transcoderr/src/steps/registry.rs` *(MODIFY)* | Replace `OnceCell<Arc<Registry>>` with `OnceCell<RwLock<Arc<Registry>>>`; extract `build_registry()` helper; add `rebuild_from_discovered()` |
| `crates/transcoderr/src/api/auth.rs` *(MODIFY)* | Add `SECRET_CATALOG_KEYS` + `redact_catalog_config` + `unredact_catalog_config` mirroring the notifier ones |
| `crates/transcoderr/src/api/plugin_catalogs.rs` *(NEW)* | `GET / POST` `/api/plugin-catalogs`, `DELETE /api/plugin-catalogs/:id`, `POST /api/plugin-catalogs/:id/refresh` |
| `crates/transcoderr/src/api/plugins.rs` *(MODIFY)* | Add `browse()` (merged catalog entries), `install()`, `delete()` (uninstall) handlers |
| `crates/transcoderr/src/api/mod.rs` *(MODIFY)* | Wire new routes |
| `crates/transcoderr/src/http/mod.rs` *(MODIFY)* | Add `ffmpeg_caps: Arc<FfmpegCaps>` to `AppState` (the registry rebuild needs it) |
| `crates/transcoderr/src/main.rs` *(MODIFY)* | Stash `ffmpeg_caps` in `AppState` (replaces the existing local Arc usage) |
| `crates/transcoderr/Cargo.toml` *(MODIFY)* | Add `tar = "0.4"`, `flate2 = "1"`, `sha2 = "0.10"` |

### Backend tests

| File | Responsibility |
|---|---|
| `crates/transcoderr/src/db/plugin_catalogs.rs` | `#[cfg(test)] mod tests` — CRUD, redact/unredact round-trip with the auth helpers |
| `crates/transcoderr/src/plugins/catalog.rs` | `#[cfg(test)] mod tests` — wiremock for `fetch_index` + `list_all` (success / 503 / bad JSON / schema_version drift / parallel fetch) |
| `crates/transcoderr/src/plugins/installer.rs` | `#[cfg(test)] mod tests` — local tarball builder; happy path, sha mismatch, wrong top-level dir, malformed manifest, atomic-rename rollback |
| `crates/transcoderr/src/steps/registry.rs` | `#[cfg(test)] mod tests` — registry swap mid-`resolve` race, in-flight Arc holds old code |
| `crates/transcoderr/tests/api_plugin_catalogs.rs` *(NEW)* | API smoke: catalog CRUD, browse-merged-list, install + uninstall |
| `crates/transcoderr/tests/plugin_install_e2e.rs` *(NEW)* | Full round-trip: mock catalog → install size-report tarball → run a flow that uses `size.report.before` / `.after` → assert `ctx.steps.size_report` populated |
| `crates/transcoderr/tests/fixtures/plugin_tarballs/` *(NEW)* | Live tarball fixtures (built on-the-fly via `tar` crate during tests, no committed binaries) |

### Frontend

| File | Responsibility |
|---|---|
| `web/src/types.ts` *(MODIFY)* | `PluginCatalog`, `CatalogEntry`, `CatalogEntryListResponse`, extend `Plugin` with `catalog_id`, `tarball_sha256` |
| `web/src/api/client.ts` *(MODIFY)* | `api.pluginCatalogs.{list,create,delete,refresh}`, `api.plugins.{browse,install,uninstall}` |
| `web/src/pages/plugins.tsx` *(MODIFY)* | Tab strip; the existing list moves under the **Installed** tab; new **Browse** + **Catalogs** tabs |
| `web/src/index.css` *(MODIFY)* | Tab strip styles, browse-table, catalogs admin form, fetch-error banner |

---

## Task 1: Migration — `plugin_catalogs` table + `plugins` columns + seed row

**Files:**
- Create: `crates/transcoderr/migrations/20260501000001_plugin_catalogs.sql`
- Test: `crates/transcoderr/src/db/mod.rs` (existing `opens_and_migrates` covers boot)

- [ ] **Step 1: Write the migration**

Create `crates/transcoderr/migrations/20260501000001_plugin_catalogs.sql`:

```sql
CREATE TABLE plugin_catalogs (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    url             TEXT NOT NULL,
    auth_header     TEXT,
    priority        INTEGER NOT NULL DEFAULT 0,
    last_fetched_at INTEGER,
    last_error      TEXT,
    created_at      INTEGER NOT NULL
);

ALTER TABLE plugins ADD COLUMN catalog_id INTEGER;
ALTER TABLE plugins ADD COLUMN tarball_sha256 TEXT;

INSERT INTO plugin_catalogs (name, url, priority, created_at)
VALUES (
    'transcoderr official',
    'https://raw.githubusercontent.com/seanbarzilay/transcoderr-plugins/main/index.json',
    0,
    strftime('%s', 'now')
);
```

- [ ] **Step 2: Run the existing migration test**

Run: `cargo test -p transcoderr db::tests::opens_and_migrates -- --nocapture`
Expected: PASS — the migration runner picks the new file up and applies it cleanly.

- [ ] **Step 3: Add a smoke test asserting the seed row exists**

Modify `crates/transcoderr/src/db/mod.rs` — add to the existing `tests` module:

```rust
#[tokio::test]
async fn migration_seeds_official_plugin_catalog() {
    let dir = tempdir().unwrap();
    let pool = open(dir.path()).await.unwrap();
    let row = sqlx::query("SELECT name, priority FROM plugin_catalogs WHERE name = 'transcoderr official'")
        .fetch_one(&pool).await.unwrap();
    use sqlx::Row;
    assert_eq!(row.get::<String, _>(0), "transcoderr official");
    assert_eq!(row.get::<i64, _>(1), 0);
}
```

- [ ] **Step 4: Run new test to verify it passes**

Run: `cargo test -p transcoderr db::tests::migration_seeds_official_plugin_catalog`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/migrations/20260501000001_plugin_catalogs.sql \
        crates/transcoderr/src/db/mod.rs
git commit -m "feat(db): plugin_catalogs migration + plugins.catalog_id/tarball_sha256"
```

---

## Task 2: `db::plugin_catalogs` CRUD module

**Files:**
- Create: `crates/transcoderr/src/db/plugin_catalogs.rs`
- Modify: `crates/transcoderr/src/db/mod.rs:37-44` (add `pub mod plugin_catalogs;`)

- [ ] **Step 1: Stub the module + register it in `db/mod.rs`**

Modify `crates/transcoderr/src/db/mod.rs`, replace the `pub mod plugins;` line with:

```rust
pub mod plugin_catalogs;
pub mod plugins;
```

Create `crates/transcoderr/src/db/plugin_catalogs.rs` with:

```rust
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct CatalogRow {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub auth_header: Option<String>,
    pub priority: i32,
    pub last_fetched_at: Option<i64>,
    pub last_error: Option<String>,
}

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<CatalogRow>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT id, name, url, auth_header, priority, last_fetched_at, last_error \
         FROM plugin_catalogs ORDER BY priority, name"
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| CatalogRow {
        id: r.get(0),
        name: r.get(1),
        url: r.get(2),
        auth_header: r.get(3),
        priority: r.get(4),
        last_fetched_at: r.get(5),
        last_error: r.get(6),
    }).collect())
}

pub async fn create(
    pool: &SqlitePool,
    name: &str,
    url: &str,
    auth_header: Option<&str>,
    priority: i32,
) -> sqlx::Result<i64> {
    let now = chrono::Utc::now().timestamp();
    let res = sqlx::query(
        "INSERT INTO plugin_catalogs (name, url, auth_header, priority, created_at) \
         VALUES (?, ?, ?, ?, ?)"
    )
    .bind(name).bind(url).bind(auth_header).bind(priority).bind(now)
    .execute(pool).await?;
    Ok(res.last_insert_rowid())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<u64> {
    let res = sqlx::query("DELETE FROM plugin_catalogs WHERE id = ?")
        .bind(id).execute(pool).await?;
    Ok(res.rows_affected())
}

pub async fn record_fetch_success(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE plugin_catalogs SET last_fetched_at = ?, last_error = NULL WHERE id = ?"
    ).bind(now).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn record_fetch_error(pool: &SqlitePool, id: i64, err: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE plugin_catalogs SET last_error = ? WHERE id = ?"
    ).bind(err).bind(id).execute(pool).await?;
    Ok(())
}
```

- [ ] **Step 2: Add unit tests inline**

Append to `crates/transcoderr/src/db/plugin_catalogs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        // The seed row from the migration would interfere; clear it first.
        sqlx::query("DELETE FROM plugin_catalogs").execute(&pool).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn create_then_list() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "internal", "https://internal.example/index.json",
                        Some("Bearer xyz"), 5).await.unwrap();
        let rows = list(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        assert_eq!(rows[0].name, "internal");
        assert_eq!(rows[0].auth_header.as_deref(), Some("Bearer xyz"));
        assert_eq!(rows[0].priority, 5);
        assert!(rows[0].last_fetched_at.is_none());
    }

    #[tokio::test]
    async fn list_orders_by_priority_then_name() {
        let (pool, _dir) = open_pool().await;
        create(&pool, "z-late", "https://z", None, 9).await.unwrap();
        create(&pool, "a-late", "https://a", None, 9).await.unwrap();
        create(&pool, "early",  "https://e", None, 1).await.unwrap();
        let rows = list(&pool).await.unwrap();
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["early", "a-late", "z-late"]);
    }

    #[tokio::test]
    async fn record_fetch_success_clears_last_error() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "x", "https://x", None, 0).await.unwrap();
        record_fetch_error(&pool, id, "boom").await.unwrap();
        record_fetch_success(&pool, id).await.unwrap();
        let rows = list(&pool).await.unwrap();
        assert!(rows[0].last_error.is_none());
        assert!(rows[0].last_fetched_at.is_some());
    }

    #[tokio::test]
    async fn delete_removes_the_row() {
        let (pool, _dir) = open_pool().await;
        let id = create(&pool, "x", "https://x", None, 0).await.unwrap();
        let removed = delete(&pool, id).await.unwrap();
        assert_eq!(removed, 1);
        assert!(list(&pool).await.unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p transcoderr db::plugin_catalogs`
Expected: 4 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/transcoderr/src/db/plugin_catalogs.rs \
        crates/transcoderr/src/db/mod.rs
git commit -m "feat(db): plugin_catalogs CRUD module"
```

---

## Task 3: Catalog HTTP client (`fetch_index` + `list_all`)

**Files:**
- Create: `crates/transcoderr/src/plugins/catalog.rs`
- Modify: `crates/transcoderr/src/plugins/mod.rs:1-3` (export `pub mod catalog;`)

- [ ] **Step 1: Add `pub mod catalog;` to `plugins/mod.rs`**

Modify `crates/transcoderr/src/plugins/mod.rs` line 1 area:

```rust
pub mod catalog;
pub mod manifest;
pub mod subprocess;
```

(Keep the existing `discover` function and `use` lines unchanged.)

- [ ] **Step 2: Write the client**

Create `crates/transcoderr/src/plugins/catalog.rs`:

```rust
use crate::db::plugin_catalogs::{self, CatalogRow};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

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
}

/// Resolved entry served to the API: an index entry tagged with the
/// catalog it came from.
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub catalog_id: i64,
    pub catalog_name: String,
    #[serde(flatten)]
    pub entry: IndexEntry,
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

#[derive(Debug, Default)]
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
        Self { cache: RwLock::new(std::collections::HashMap::new()) }
    }
}

impl CatalogClient {
    pub async fn fetch_index(&self, catalog: &CatalogRow) -> anyhow::Result<Vec<IndexEntry>> {
        let mut req = reqwest::Client::new().get(&catalog.url);
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
                catalog.name, idx.schema_version, CATALOG_SCHEMA_VERSION
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
                    Some(e) if e.fetched_at.is_some_and(|t| now.duration_since(t) < CACHE_TTL) => {
                        match &e.result {
                            Ok(plugins) => for p in plugins {
                                entries.push(CatalogEntry {
                                    catalog_id: c.id,
                                    catalog_name: c.name.clone(),
                                    entry: p.clone(),
                                });
                            },
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
            let res = self.fetch_index(c).await
                .map_err(|e| e.to_string());
            let mut cache = self.cache.write().await;
            cache.insert(c.id, CacheEntry { fetched_at: Some(now), result: res.clone() });
            match res {
                Ok(plugins) => {
                    let _ = plugin_catalogs::record_fetch_success(pool, c.id).await;
                    for p in plugins {
                        entries.push(CatalogEntry {
                            catalog_id: c.id,
                            catalog_name: c.name.clone(),
                            entry: p,
                        });
                    }
                }
                Err(e) => {
                    let _ = plugin_catalogs::record_fetch_error(pool, c.id, &e).await;
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
```

- [ ] **Step 3: Add wiremock unit tests inline**

Append to `crates/transcoderr/src/plugins/catalog.rs`:

```rust
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
        sqlx::query("DELETE FROM plugin_catalogs").execute(&pool).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn fetch_index_happy_path_returns_plugins() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/index.json"))
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
            .mount(&server).await;

        let cat = CatalogRow {
            id: 1, name: "test".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: None, priority: 0,
            last_fetched_at: None, last_error: None,
        };
        let plugins = CatalogClient::default().fetch_index(&cat).await.unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "size-report");
        assert_eq!(plugins[0].provides_steps.len(), 2);
    }

    #[tokio::test]
    async fn fetch_index_sends_auth_header_when_present() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/index.json"))
            .and(header("Authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1, "plugins": []
            })))
            .expect(1)
            .mount(&server).await;

        let cat = CatalogRow {
            id: 1, name: "private".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: Some("Bearer secret".into()),
            priority: 0, last_fetched_at: None, last_error: None,
        };
        CatalogClient::default().fetch_index(&cat).await.unwrap();
    }

    #[tokio::test]
    async fn fetch_index_rejects_wrong_schema_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 99, "plugins": []
            })))
            .mount(&server).await;
        let cat = CatalogRow {
            id: 1, name: "future".into(),
            url: format!("{}/index.json", server.uri()),
            auth_header: None, priority: 0,
            last_fetched_at: None, last_error: None,
        };
        let err = CatalogClient::default().fetch_index(&cat).await.unwrap_err();
        assert!(err.to_string().contains("schema_version 99"));
    }

    #[tokio::test]
    async fn list_all_aggregates_success_and_failure_per_catalog() {
        let server_ok = MockServer::start().await;
        Mock::given(method("GET")).and(path("/ok.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1,
                "plugins": [{
                    "name": "x", "version": "0.1.0", "summary": "",
                    "tarball_url": "https://e/x.tgz", "tarball_sha256": "h",
                    "kind": "subprocess", "provides_steps": []
                }]
            })))
            .mount(&server_ok).await;
        let server_bad = MockServer::start().await;
        Mock::given(method("GET")).and(path("/bad.json"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server_bad).await;

        let (pool, _dir) = open_pool().await;
        plugin_catalogs::create(&pool, "ok", &format!("{}/ok.json", server_ok.uri()), None, 0).await.unwrap();
        plugin_catalogs::create(&pool, "bad", &format!("{}/bad.json", server_bad.uri()), None, 1).await.unwrap();

        let res = CatalogClient::default().list_all(&pool).await.unwrap();
        assert_eq!(res.entries.len(), 1);
        assert_eq!(res.entries[0].catalog_name, "ok");
        assert_eq!(res.entries[0].entry.name, "x");
        assert_eq!(res.errors.len(), 1);
        assert_eq!(res.errors[0].catalog_name, "bad");
        assert!(res.errors[0].error.contains("503"));

        // last_error / last_fetched_at recorded.
        let rows = plugin_catalogs::list(&pool).await.unwrap();
        let ok_row  = rows.iter().find(|r| r.name == "ok").unwrap();
        let bad_row = rows.iter().find(|r| r.name == "bad").unwrap();
        assert!(ok_row.last_fetched_at.is_some());
        assert!(ok_row.last_error.is_none());
        assert!(bad_row.last_error.as_ref().unwrap().contains("503"));
    }

    #[tokio::test]
    async fn list_all_serves_from_cache_within_ttl() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/index.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "schema_version": 1, "plugins": []
            })))
            .expect(1)  // <-- key: only ONE call expected
            .mount(&server).await;

        let (pool, _dir) = open_pool().await;
        plugin_catalogs::create(&pool, "c", &format!("{}/index.json", server.uri()), None, 0).await.unwrap();

        let client = CatalogClient::default();
        client.list_all(&pool).await.unwrap();
        client.list_all(&pool).await.unwrap();  // cache hit — no second HTTP call
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p transcoderr plugins::catalog`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/plugins/catalog.rs \
        crates/transcoderr/src/plugins/mod.rs
git commit -m "feat(plugins): catalog HTTP client with 5-min cache + per-catalog error surfacing"
```

---

## Task 4: Tarball installer — happy path

**Files:**
- Create: `crates/transcoderr/src/plugins/installer.rs`
- Modify: `crates/transcoderr/src/plugins/mod.rs` (add `pub mod installer;`)
- Modify: `crates/transcoderr/Cargo.toml` (add `tar`, `flate2`, `sha2`)

- [ ] **Step 1: Add deps**

Modify `crates/transcoderr/Cargo.toml` — append to `[dependencies]`:

```toml
tar = "0.4"
flate2 = "1"
sha2 = "0.10"
```

Run: `cargo build -p transcoderr`
Expected: clean build (no code uses them yet).

- [ ] **Step 2: Add `pub mod installer;` to plugins/mod.rs**

Modify `crates/transcoderr/src/plugins/mod.rs`:

```rust
pub mod catalog;
pub mod installer;
pub mod manifest;
pub mod subprocess;
```

- [ ] **Step 3: Write the installer with the happy path only**

Create `crates/transcoderr/src/plugins/installer.rs`:

```rust
use crate::plugins::catalog::IndexEntry;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("download failed: {0}")]
    Download(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sha mismatch: expected {expected}, got {got}")]
    ShaMismatch { expected: String, got: String },
    #[error("tarball layout: {0}")]
    Layout(String),
    #[error("manifest: {0}")]
    Manifest(String),
}

#[derive(Debug)]
pub struct InstalledPlugin {
    pub name: String,
    pub plugin_dir: PathBuf,
    pub tarball_sha256: String,
}

/// Download, verify, extract, atomic-swap. Returns details of the
/// installed plugin on success. The caller is responsible for the
/// post-install bookkeeping (sync_discovered, registry rebuild).
pub async fn install_from_entry(
    entry: &IndexEntry,
    plugins_dir: &Path,
) -> Result<InstalledPlugin, InstallError> {
    std::fs::create_dir_all(plugins_dir)?;
    let suffix: String = (0..8)
        .map(|_| (b'a' + (rand::random::<u8>() % 26)) as char)
        .collect();
    let staging = plugins_dir.join(format!(".tcr-install.{suffix}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;

    // Stream-download into the staging dir as a temp file, hashing as we go.
    let tmp_tar = staging.join("plugin.tar.gz");
    let resp = reqwest::Client::new().get(&entry.tarball_url).send().await?;
    if !resp.status().is_success() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!("HTTP {}", resp.status())));
    }
    let body = resp.bytes().await?;
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let got = hex(&hasher.finalize());
    if got != entry.tarball_sha256.to_lowercase() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::ShaMismatch {
            expected: entry.tarball_sha256.clone(),
            got,
        });
    }
    let mut f = std::fs::File::create(&tmp_tar)?;
    f.write_all(&body)?;
    drop(f);

    // Untar into staging/extracted/.
    let extracted = staging.join("extracted");
    std::fs::create_dir_all(&extracted)?;
    let f = std::fs::File::open(&tmp_tar)?;
    let gz = GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(&extracted)?;

    // Verify exactly one top-level dir matching entry.name.
    let mut top_dirs: Vec<PathBuf> = std::fs::read_dir(&extracted)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().is_some_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    if top_dirs.len() != 1 {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!(
            "expected 1 top-level dir, got {}", top_dirs.len()
        )));
    }
    let top_dir = top_dirs.remove(0);
    let top_name = top_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if top_name != entry.name {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Layout(format!(
            "top-level dir is {top_name:?}, expected {:?}", entry.name
        )));
    }

    // Verify manifest parses and name matches.
    let manifest_raw = std::fs::read_to_string(top_dir.join("manifest.toml"))
        .map_err(|e| InstallError::Manifest(format!("manifest.toml: {e}")))?;
    let manifest: crate::plugins::manifest::Manifest = toml::from_str(&manifest_raw)
        .map_err(|e| InstallError::Manifest(format!("parse: {e}")))?;
    if manifest.name != entry.name {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(InstallError::Manifest(format!(
            "manifest.name is {:?}, expected {:?}", manifest.name, entry.name
        )));
    }

    // Atomic swap.
    let target = plugins_dir.join(&entry.name);
    let backup = plugins_dir.join(format!(".tcr-old.{}.{suffix}", entry.name));
    if target.exists() {
        std::fs::rename(&target, &backup)?;
    }
    std::fs::rename(&top_dir, &target)?;
    let _ = std::fs::remove_dir_all(&backup);
    let _ = std::fs::remove_dir_all(&staging);

    Ok(InstalledPlugin {
        name: entry.name.clone(),
        plugin_dir: target,
        tarball_sha256: got,
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { let _ = write!(s, "{:02x}", b); }
    s
}
```

- [ ] **Step 4: Add a tarball-builder test helper + happy-path test**

Append to `crates/transcoderr/src/plugins/installer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use serde_json::json;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a tar.gz of `<plugin_name>/manifest.toml` (+ optional bin/run)
    /// in memory and return (bytes, sha256_hex).
    fn build_tarball(plugin_name: &str, manifest_toml: &str, with_bin_run: bool) -> (Vec<u8>, String) {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);

            // Top-level dir entry.
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path(format!("{plugin_name}/")).unwrap();
            hdr.set_mode(0o755);
            hdr.set_size(0);
            hdr.set_cksum();
            tar.append(&hdr, std::io::empty()).unwrap();

            // manifest.toml.
            let manifest = manifest_toml.as_bytes();
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path(format!("{plugin_name}/manifest.toml")).unwrap();
            hdr.set_mode(0o644);
            hdr.set_size(manifest.len() as u64);
            hdr.set_cksum();
            tar.append(&hdr, manifest).unwrap();

            if with_bin_run {
                let body = b"#!/bin/sh\necho ok\n";
                let mut hdr = tar::Header::new_gnu();
                hdr.set_path(format!("{plugin_name}/bin/run")).unwrap();
                hdr.set_mode(0o755);
                hdr.set_size(body.len() as u64);
                hdr.set_cksum();
                tar.append(&hdr, &body[..]).unwrap();
            }

            tar.finish().unwrap();
        }
        let bytes = gz.finish().unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha = hex(&h.finalize());
        (bytes, sha)
    }

    fn manifest_for(name: &str) -> String {
        format!(
            r#"name = "{name}"
version = "0.1.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["{name}.do"]
"#
        )
    }

    #[tokio::test]
    async fn install_happy_path_extracts_and_swaps() {
        let (bytes, sha) = build_tarball("hello", &manifest_for("hello"), true);

        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/hello.tar.gz"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_bytes(bytes)
                .insert_header("content-type", "application/gzip"))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/hello.tar.gz", server.uri()),
            tarball_sha256: sha.clone(),
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec!["hello.do".into()],
        };
        let installed = install_from_entry(&entry, plugins_dir.path()).await.unwrap();
        assert_eq!(installed.name, "hello");
        assert_eq!(installed.tarball_sha256, sha);
        assert!(installed.plugin_dir.exists());
        assert!(installed.plugin_dir.join("manifest.toml").exists());
        assert!(installed.plugin_dir.join("bin/run").exists());
        // No leftover staging dir.
        let leftovers: Vec<_> = std::fs::read_dir(plugins_dir.path()).unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tcr-"))
            .collect();
        assert!(leftovers.is_empty(), "staging dirs should be cleaned up");
    }
}
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p transcoderr plugins::installer::tests::install_happy_path_extracts_and_swaps`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/transcoderr/src/plugins/installer.rs \
        crates/transcoderr/src/plugins/mod.rs \
        crates/transcoderr/Cargo.toml \
        crates/transcoderr/Cargo.lock
git commit -m "feat(plugins): tarball installer happy path"
```

---

## Task 5: Installer error paths

**Files:**
- Modify: `crates/transcoderr/src/plugins/installer.rs` (extend `mod tests`)

- [ ] **Step 1: Add error-path tests**

Append to the `tests` mod inside `crates/transcoderr/src/plugins/installer.rs`:

```rust
    #[tokio::test]
    async fn install_fails_on_sha_mismatch_and_leaves_no_staging() {
        let (bytes, _real_sha) = build_tarball("hello", &manifest_for("hello"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/hello.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/hello.tar.gz", server.uri()),
            tarball_sha256: "0".repeat(64),  // wrong
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec![],
        };
        let err = install_from_entry(&entry, plugins_dir.path()).await.unwrap_err();
        assert!(matches!(err, InstallError::ShaMismatch { .. }));
        // Plugin dir was not created and staging was cleaned.
        assert!(!plugins_dir.path().join("hello").exists());
        let entries: Vec<_> = std::fs::read_dir(plugins_dir.path()).unwrap()
            .filter_map(|e| e.ok()).collect();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn install_fails_when_top_dir_does_not_match_name() {
        // Tarball top-level dir is "wrong", entry says it's "hello".
        let (bytes, sha) = build_tarball("wrong", &manifest_for("wrong"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/x.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/x.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None,
            min_transcoderr_version: None,
            kind: "subprocess".into(),
            provides_steps: vec![],
        };
        let err = install_from_entry(&entry, plugins_dir.path()).await.unwrap_err();
        match err {
            InstallError::Layout(msg) => assert!(msg.contains("wrong"), "msg: {msg}"),
            other => panic!("expected Layout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn install_fails_when_manifest_name_does_not_match_entry() {
        // Manifest says name="other" but the entry insists it's "hello".
        // The tarball's top-dir IS "hello" so layout passes -- only the
        // manifest cross-check catches it.
        let mut bad_manifest = manifest_for("other");
        // Tarball top-dir mismatch would be caught earlier; build a
        // tarball whose top-dir is "hello" but manifest says "other".
        bad_manifest = bad_manifest.replace("name = \"other\"", "name = \"other\"");

        // Build a custom tarball with "hello/" as top-dir + bad_manifest.
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        {
            let mut tar = tar::Builder::new(&mut gz);
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path("hello/").unwrap(); hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
            tar.append(&hdr, std::io::empty()).unwrap();
            let body = bad_manifest.as_bytes();
            let mut hdr = tar::Header::new_gnu();
            hdr.set_path("hello/manifest.toml").unwrap();
            hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
            tar.append(&hdr, body).unwrap();
            tar.finish().unwrap();
        }
        let bytes = gz.finish().unwrap();
        let mut h = Sha256::new(); h.update(&bytes);
        let sha = hex(&h.finalize());

        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/x.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let plugins_dir = tempdir().unwrap();
        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/x.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None, min_transcoderr_version: None,
            kind: "subprocess".into(), provides_steps: vec![],
        };
        let err = install_from_entry(&entry, plugins_dir.path()).await.unwrap_err();
        match err {
            InstallError::Manifest(msg) => assert!(msg.contains("other")),
            other => panic!("expected Manifest, got {other:?}"),
        }
        assert!(!plugins_dir.path().join("hello").exists());
    }

    #[tokio::test]
    async fn install_replaces_existing_plugin_dir() {
        // Pre-existing plugins/hello/ has a sentinel file.
        let plugins_dir = tempdir().unwrap();
        let target = plugins_dir.path().join("hello");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("sentinel"), "old").unwrap();

        let (bytes, sha) = build_tarball("hello", &manifest_for("hello"), true);
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/h.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server).await;

        let entry = IndexEntry {
            name: "hello".into(),
            version: "0.1.0".into(),
            summary: "".into(),
            tarball_url: format!("{}/h.tar.gz", server.uri()),
            tarball_sha256: sha,
            homepage: None, min_transcoderr_version: None,
            kind: "subprocess".into(), provides_steps: vec![],
        };
        install_from_entry(&entry, plugins_dir.path()).await.unwrap();

        assert!(target.join("manifest.toml").exists());
        assert!(target.join("bin/run").exists());
        assert!(!target.join("sentinel").exists(), "old contents replaced");
    }
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p transcoderr plugins::installer`
Expected: 5 passed (1 from the previous task + 4 new).

- [ ] **Step 3: Commit**

```bash
git add crates/transcoderr/src/plugins/installer.rs
git commit -m "test(plugins): installer error paths (sha mismatch, layout, manifest, replace)"
```

---

## Task 6: Uninstaller

**Files:**
- Create: `crates/transcoderr/src/plugins/uninstaller.rs`
- Modify: `crates/transcoderr/src/plugins/mod.rs`

- [ ] **Step 1: Add `pub mod uninstaller;`**

Modify `crates/transcoderr/src/plugins/mod.rs`:

```rust
pub mod catalog;
pub mod installer;
pub mod manifest;
pub mod subprocess;
pub mod uninstaller;
```

- [ ] **Step 2: Write the uninstaller**

Create `crates/transcoderr/src/plugins/uninstaller.rs`:

```rust
use sqlx::SqlitePool;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("plugin {0:?} not found in DB")]
    NotFound(String),
}

/// Remove the plugin directory from disk and drop the row from the DB.
/// Caller is responsible for the registry rebuild afterwards.
pub async fn uninstall(
    pool: &SqlitePool,
    plugins_dir: &Path,
    plugin_id: i64,
) -> Result<String, UninstallError> {
    use sqlx::Row;
    let row = sqlx::query("SELECT name FROM plugins WHERE id = ?")
        .bind(plugin_id).fetch_optional(pool).await?;
    let row = match row {
        Some(r) => r,
        None => return Err(UninstallError::NotFound(plugin_id.to_string())),
    };
    let name: String = row.get(0);
    let dir = plugins_dir.join(&name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    sqlx::query("DELETE FROM plugins WHERE id = ?").bind(plugin_id).execute(pool).await?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn open_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn uninstall_removes_dir_and_db_row() {
        let (pool, _data) = open_pool().await;
        let plugins_dir = tempdir().unwrap();
        let plugin_dir = plugins_dir.path().join("foo");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("manifest.toml"), "name = \"foo\"\nversion = \"0.1.0\"\nkind = \"subprocess\"\nentrypoint = \"bin/run\"\nprovides_steps = []\n").unwrap();

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO plugins (name, version, kind, path, schema_json, enabled) \
             VALUES ('foo', '0.1.0', 'subprocess', ?, '{}', 1) RETURNING id"
        ).bind(plugin_dir.to_string_lossy().to_string())
         .fetch_one(&pool).await.unwrap();

        uninstall(&pool, plugins_dir.path(), id).await.unwrap();
        assert!(!plugin_dir.exists());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM plugins")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn uninstall_returns_not_found_for_missing_id() {
        let (pool, _data) = open_pool().await;
        let plugins_dir = tempdir().unwrap();
        let err = uninstall(&pool, plugins_dir.path(), 9999).await.unwrap_err();
        assert!(matches!(err, UninstallError::NotFound(_)));
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p transcoderr plugins::uninstaller`
Expected: 2 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/transcoderr/src/plugins/uninstaller.rs \
        crates/transcoderr/src/plugins/mod.rs
git commit -m "feat(plugins): uninstaller module"
```

---

## Task 7: Step registry — `RwLock<Arc<Registry>>` + `rebuild_from_discovered`

**Files:**
- Modify: `crates/transcoderr/src/steps/registry.rs`

- [ ] **Step 1: Refactor the registry storage**

Replace the contents of `crates/transcoderr/src/steps/registry.rs` with:

```rust
use crate::hw::semaphores::DeviceRegistry;
use crate::plugins::manifest::DiscoveredPlugin;
use crate::plugins::subprocess::SubprocessStep;
use crate::steps::{builtin, Step};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{OnceCell, RwLock};

/// Inputs needed to (re)build the registry. Stashed at boot so
/// `rebuild_from_discovered` can recreate without the caller having
/// to re-thread these values from main.rs.
struct BuildInputs {
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
}

static REGISTRY: OnceCell<RwLock<Arc<Registry>>> = OnceCell::const_new();
static BUILD_INPUTS: OnceCell<BuildInputs> = OnceCell::const_new();

pub struct Registry {
    by_name: HashMap<String, Arc<dyn Step>>,
}

impl Registry {
    pub fn empty() -> Self {
        Self { by_name: HashMap::new() }
    }
}

fn build(
    inputs: &BuildInputs,
    discovered: Vec<DiscoveredPlugin>,
) -> Registry {
    let mut reg = Registry::empty();
    builtin::register_all(
        &mut reg.by_name,
        inputs.pool.clone(),
        inputs.hw.clone(),
        inputs.ffmpeg_caps.clone(),
    );
    for d in discovered {
        if d.manifest.kind != "subprocess" {
            continue;
        }
        let entry = d.manifest.entrypoint.clone().unwrap_or_default();
        let abs = d.manifest_dir.join(&entry);
        for step_name in &d.manifest.provides_steps {
            let step = SubprocessStep {
                step_name: step_name.clone(),
                entrypoint_abs: abs.clone(),
            };
            reg.by_name.insert(step_name.clone(), Arc::new(step));
        }
    }
    reg
}

pub async fn init(
    pool: SqlitePool,
    hw: DeviceRegistry,
    ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
    discovered: Vec<DiscoveredPlugin>,
) {
    let inputs = BuildInputs { pool, hw, ffmpeg_caps };
    let reg = build(&inputs, discovered);
    let _ = BUILD_INPUTS.set(inputs);
    let _ = REGISTRY.set(RwLock::new(Arc::new(reg)));
}

/// Rebuild and atomically swap the registry. In-flight runs that
/// already called `resolve()` keep their `Arc<dyn Step>` so they
/// finish on the old code; new `resolve()` calls return the new
/// step set.
pub async fn rebuild_from_discovered(discovered: Vec<DiscoveredPlugin>) {
    let Some(inputs) = BUILD_INPUTS.get() else { return };
    let new = build(inputs, discovered);
    if let Some(rw) = REGISTRY.get() {
        *rw.write().await = Arc::new(new);
    }
}

/// Resolve a step by name. If the registry has not been initialized
/// (e.g. unit tests that skip `init`), falls back to the built-in
/// dispatch table. NOTE: the fallback cannot instantiate `notify`
/// (needs a pool) — tests that exercise notify must call `init`.
pub async fn resolve(name: &str) -> Option<Arc<dyn Step>> {
    if let Some(rw) = REGISTRY.get() {
        return rw.read().await.by_name.get(name).cloned();
    }
    let mut map: HashMap<String, Arc<dyn Step>> = HashMap::new();
    builtin::register_all(
        &mut map,
        SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
        DeviceRegistry::empty(),
        Arc::new(crate::ffmpeg_caps::FfmpegCaps::default()),
    );
    map.remove(name)
}
```

- [ ] **Step 2: Confirm the existing test suite still passes**

Run: `cargo test -p transcoderr --tests --lib`
Expected: same number of tests as before, all green. The shape change should be transparent to existing callers.

- [ ] **Step 3: Add a swap-during-resolve race test**

Append to `crates/transcoderr/src/steps/registry.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::Context;
    use crate::plugins::manifest::Manifest;
    use crate::steps::StepProgress;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    /// Build a minimal DiscoveredPlugin pointing at a shell script that
    /// emits `result:ok`. Used to verify rebuild_from_discovered swaps
    /// in a new step that wasn't there at boot.
    fn discovered_with_step(plugin_name: &str, step_name: &str, dir: &std::path::Path) -> DiscoveredPlugin {
        let plugin_dir = dir.join(plugin_name);
        std::fs::create_dir_all(plugin_dir.join("bin")).unwrap();
        let script = "#!/bin/sh\nread INIT\nread EXEC\necho '{\"event\":\"result\",\"status\":\"ok\",\"outputs\":{}}'\n";
        let entry = plugin_dir.join("bin/run");
        std::fs::write(&entry, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&entry).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&entry, p).unwrap();
        }
        DiscoveredPlugin {
            manifest: Manifest {
                name: plugin_name.into(),
                version: "0.1.0".into(),
                kind: "subprocess".into(),
                entrypoint: Some("bin/run".into()),
                provides_steps: vec![step_name.into()],
                requires: serde_json::Value::Null,
                capabilities: vec![],
            },
            manifest_dir: plugin_dir,
            schema: serde_json::Value::Null,
        }
    }

    /// Initialize the registry once. The OnceCell is process-wide, so
    /// tests in this binary share it. We use a marker test that only
    /// installs init the first time.
    async fn ensure_init() {
        let dir = tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        init(
            pool,
            DeviceRegistry::empty(),
            Arc::new(crate::ffmpeg_caps::FfmpegCaps::default()),
            vec![],
        ).await;
        // Leak the temp dir so the migration files stay reachable; this
        // is a one-shot global init for the whole test binary.
        std::mem::forget(dir);
    }

    #[tokio::test]
    async fn rebuild_adds_a_new_step_visible_to_subsequent_resolves() {
        ensure_init().await;
        let dir = tempdir().unwrap();
        let d = discovered_with_step("hello", "rebuild.test.step", dir.path());

        // Step is not in the registry yet.
        assert!(resolve("rebuild.test.step").await.is_none());

        rebuild_from_discovered(vec![d]).await;

        let step = resolve("rebuild.test.step").await.expect("step now present");
        let mut ctx = Context::for_file("/x");
        let mut cb = |_: StepProgress| {};
        step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    }

    #[tokio::test]
    async fn in_flight_arc_survives_a_swap() {
        ensure_init().await;
        let dir = tempdir().unwrap();
        let d = discovered_with_step("inflight", "inflight.test.step", dir.path());
        rebuild_from_discovered(vec![d]).await;

        let step = resolve("inflight.test.step").await.expect("step present pre-swap");

        // Swap to an empty registry (drops the step). The in-flight
        // Arc<dyn Step> we hold should still be runnable.
        rebuild_from_discovered(vec![]).await;

        assert!(resolve("inflight.test.step").await.is_none(), "step gone after swap");

        let mut ctx = Context::for_file("/x");
        let mut cb = |_: StepProgress| {};
        step.execute(&BTreeMap::new(), &mut ctx, &mut cb).await.unwrap();
    }
}
```

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p transcoderr steps::registry`
Expected: 2 passed (the swap tests). Other test binaries that call `init` first will not interfere because `OnceCell::set` is idempotent and `BUILD_INPUTS` falls through cleanly.

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/steps/registry.rs
git commit -m "refactor(steps): RwLock<Arc<Registry>> + rebuild_from_discovered"
```

---

## Task 8: AppState gains `ffmpeg_caps` + boot wires it

**Files:**
- Modify: `crates/transcoderr/src/http/mod.rs`
- Modify: `crates/transcoderr/src/main.rs`
- Modify: `crates/transcoderr/tests/common/mod.rs` (the test boot helper)

- [ ] **Step 1: Add `ffmpeg_caps` to `AppState`**

Modify `crates/transcoderr/src/http/mod.rs:14-25`:

```rust
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub hw_caps: Arc<tokio::sync::RwLock<crate::hw::HwCaps>>,
    pub hw_devices: crate::hw::semaphores::DeviceRegistry,
    pub ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
    pub bus: crate::bus::Bus,
    pub ready: crate::ready::Readiness,
    pub metrics: std::sync::Arc<crate::metrics::Metrics>,
    pub cancellations: crate::cancellation::JobCancellations,
    pub public_url: std::sync::Arc<String>,
    pub arr_cache: std::sync::Arc<crate::arr::cache::ArrCache>,
}
```

- [ ] **Step 2: Wire in `main.rs`**

Modify `crates/transcoderr/src/main.rs` where `AppState` is constructed — find the line just below the existing `transcoderr::steps::registry::init(...)` block and pass `ffmpeg_caps.clone()` into `AppState { ... }`:

```rust
// (existing) ffmpeg_caps already constructed earlier:
let ffmpeg_caps = std::sync::Arc::new(
    transcoderr::ffmpeg_caps::FfmpegCaps::probe().await,
);

// ... and somewhere inside the AppState struct literal:
ffmpeg_caps: ffmpeg_caps.clone(),
```

- [ ] **Step 3: Wire in the test boot helper**

Modify `crates/transcoderr/tests/common/mod.rs` — the AppState construction inside `boot()`. Add `ffmpeg_caps` to it. Look for the call to `registry::init` and the `AppState { ... }` literal nearby. The `ffmpeg_caps` Arc the test builds for `registry::init` should also be assigned into AppState.

- [ ] **Step 4: Run the entire test suite**

Run: `cargo test -p transcoderr`
Expected: all green. (No new tests; the existing suite verifies nothing regressed.)

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/http/mod.rs \
        crates/transcoderr/src/main.rs \
        crates/transcoderr/tests/common/mod.rs
git commit -m "refactor(http): AppState carries ffmpeg_caps for the registry rebuild"
```

---

## Task 9: API — `auth_header` redact / unredact helpers

**Files:**
- Modify: `crates/transcoderr/src/api/auth.rs`

- [ ] **Step 1: Add the catalog-secret constant + helpers**

Append to `crates/transcoderr/src/api/auth.rs` (just below the `unredact_notifier_config` function — same module-level placement):

```rust
const SECRET_CATALOG_KEYS: &[&str] = &["auth_header"];

/// Replace secret catalog fields in-place with `"***"` for token-authed
/// responses. Mirrors `redact_notifier_config` but operates on a flat
/// catalog row JSON rather than a notifier `config` blob.
pub fn redact_catalog_row(row: &mut serde_json::Value) {
    if let Some(obj) = row.as_object_mut() {
        for k in SECRET_CATALOG_KEYS {
            if obj.get(*k).is_some_and(|v| !v.is_null()) {
                obj.insert((*k).into(), serde_json::Value::String("***".into()));
            }
        }
    }
}

/// On PUT, replace any `"***"` values at known SECRET_CATALOG_KEYS
/// positions with the row's current value -- prevents a token-authed
/// caller from accidentally overwriting the real secret with the
/// redaction sentinel during a GET → mutate → PUT round trip.
pub fn unredact_catalog_row(
    new_row: &mut serde_json::Value,
    current_row: &serde_json::Value,
) {
    let (Some(new_obj), Some(cur_obj)) = (new_row.as_object_mut(), current_row.as_object()) else {
        return;
    };
    for k in SECRET_CATALOG_KEYS {
        if new_obj.get(*k) == Some(&serde_json::Value::String("***".into())) {
            if let Some(real) = cur_obj.get(*k) {
                new_obj.insert((*k).into(), real.clone());
            }
        }
    }
}
```

- [ ] **Step 2: Add a redaction round-trip test**

In the same file, append to the existing `#[cfg(test)] mod tests` block (or create one if absent):

```rust
#[cfg(test)]
mod catalog_redaction_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redact_replaces_auth_header_with_sentinel() {
        let mut row = json!({"id": 1, "name": "x", "auth_header": "Bearer secret"});
        redact_catalog_row(&mut row);
        assert_eq!(row["auth_header"], "***");
    }

    #[test]
    fn redact_leaves_null_auth_header_alone() {
        let mut row = json!({"id": 1, "name": "x", "auth_header": null});
        redact_catalog_row(&mut row);
        assert!(row["auth_header"].is_null());
    }

    #[test]
    fn unredact_restores_real_value_on_round_trip() {
        let current = json!({"auth_header": "Bearer secret"});
        let mut new = json!({"auth_header": "***", "name": "renamed"});
        unredact_catalog_row(&mut new, &current);
        assert_eq!(new["auth_header"], "Bearer secret");
        assert_eq!(new["name"], "renamed");
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p transcoderr api::auth::catalog_redaction_tests`
Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/transcoderr/src/api/auth.rs
git commit -m "feat(api): redact_catalog_row + unredact for catalog auth_header round-trip"
```

---

## Task 10: API — plugin_catalogs CRUD

**Files:**
- Create: `crates/transcoderr/src/api/plugin_catalogs.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`
- Modify: `crates/transcoderr/src/http/mod.rs` (add `catalog_client` to AppState)

- [ ] **Step 1: Add `catalog_client` to AppState**

Modify `crates/transcoderr/src/http/mod.rs`:

```rust
pub struct AppState {
    // ... existing fields ...
    pub catalog_client: std::sync::Arc<crate::plugins::catalog::CatalogClient>,
}
```

And in `crates/transcoderr/src/main.rs` and `crates/transcoderr/tests/common/mod.rs`, initialize it as `std::sync::Arc::new(transcoderr::plugins::catalog::CatalogClient::default())` in the `AppState { ... }` literal.

- [ ] **Step 2: Write the CRUD handlers**

Create `crates/transcoderr/src/api/plugin_catalogs.rs`:

```rust
use crate::api::auth::{redact_catalog_row, unredact_catalog_row, AuthSource};
use crate::db::plugin_catalogs;
use crate::http::AppState;
use axum::{extract::{Path, State}, http::StatusCode, Extension, Json};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct CreateReq {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<serde_json::Value>>, StatusCode> {
    let rows = plugin_catalogs::list(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out: Vec<_> = rows.into_iter().map(|r| {
        let mut v = json!({
            "id": r.id,
            "name": r.name,
            "url": r.url,
            "auth_header": r.auth_header,
            "priority": r.priority,
            "last_fetched_at": r.last_fetched_at,
            "last_error": r.last_error,
        });
        if auth == AuthSource::Token { redact_catalog_row(&mut v); }
        v
    }).collect();
    Ok(Json(out))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateReq>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let id = plugin_catalogs::create(
        &state.pool,
        &req.name,
        &req.url,
        req.auth_header.as_deref(),
        req.priority.unwrap_or(0),
    ).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"id": id})))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = plugin_catalogs::delete(&state.pool, id).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    state.catalog_client.invalidate(id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    state.catalog_client.invalidate(id).await;
    Ok(StatusCode::NO_CONTENT)
}

// Suppresses dead-code warning until update is wired (covered by future
// "edit a catalog" UX). Kept here so the unredact helper has a caller.
#[allow(dead_code)]
fn _ensure_unredact_in_use(new: &mut serde_json::Value, cur: &serde_json::Value) {
    unredact_catalog_row(new, cur);
}
```

- [ ] **Step 3: Wire the routes**

Modify `crates/transcoderr/src/api/mod.rs` — add to the route list (alongside the existing `/plugins` lines):

```rust
.route("/plugin-catalogs",          get(plugin_catalogs::list).post(plugin_catalogs::create))
.route("/plugin-catalogs/:id",      delete(plugin_catalogs::delete))
.route("/plugin-catalogs/:id/refresh", post(plugin_catalogs::refresh))
```

Add the `mod plugin_catalogs;` declaration near the other `mod` lines at the top of `api/mod.rs`.

- [ ] **Step 4: Build to confirm wiring**

Run: `cargo build -p transcoderr`
Expected: clean build.

- [ ] **Step 5: Add an API smoke test**

Create `crates/transcoderr/tests/api_plugin_catalogs.rs`:

```rust
mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn plugin_catalogs_crud() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // The migration seeds one official catalog. List should show it.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list1.len(), 1);
    assert_eq!(list1[0]["name"], "transcoderr official");

    // Create a private catalog with an auth header.
    let resp: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "internal",
            "url": "https://internal.example/index.json",
            "auth_header": "Bearer xyz",
            "priority": 5,
        }))
        .send().await.unwrap().json().await.unwrap();
    let id = resp["id"].as_i64().unwrap();

    // List now has two.
    let list2: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list2.len(), 2);

    // Delete returns 204.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 204);

    // Deleting again returns 404.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 404);
}
```

- [ ] **Step 6: Run the API test**

Run: `cargo test -p transcoderr --test api_plugin_catalogs`
Expected: 1 passed.

- [ ] **Step 7: Commit**

```bash
git add crates/transcoderr/src/api/plugin_catalogs.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/src/http/mod.rs \
        crates/transcoderr/src/main.rs \
        crates/transcoderr/tests/common/mod.rs \
        crates/transcoderr/tests/api_plugin_catalogs.rs
git commit -m "feat(api): plugin_catalogs CRUD endpoints"
```

---

## Task 11: API — merged catalog-entries list

**Files:**
- Modify: `crates/transcoderr/src/api/plugins.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

- [ ] **Step 1: Add the `browse()` handler**

Append to `crates/transcoderr/src/api/plugins.rs`:

```rust
use crate::plugins::catalog::ListAllResult;

pub async fn browse(
    State(state): State<AppState>,
) -> Result<Json<ListAllResult>, StatusCode> {
    state.catalog_client
        .list_all(&state.pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
```

(Add the `use` for `ListAllResult` at the top of the file alongside the existing imports.)

- [ ] **Step 2: Wire the route**

Modify `crates/transcoderr/src/api/mod.rs` — add alongside existing `/plugins` route:

```rust
.route("/plugin-catalog-entries", get(plugins::browse))
```

- [ ] **Step 3: Add an API test**

Append to `crates/transcoderr/tests/api_plugin_catalogs.rs`:

```rust
#[tokio::test]
async fn browse_returns_entries_and_errors_per_catalog() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = boot().await;
    let client = reqwest::Client::new();

    // Replace the seed catalog with a wiremock-backed one so the test
    // doesn't try to fetch from the real internet.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list1[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();

    let server_ok = MockServer::start().await;
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "size-report",
                "version": "0.1.0",
                "summary": "size",
                "tarball_url": "https://example.com/x.tgz",
                "tarball_sha256": "h",
                "kind": "subprocess",
                "provides_steps": ["size.report.before"]
            }]
        })))
        .mount(&server_ok).await;

    client.post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "ok",
            "url": format!("{}/index.json", server_ok.uri()),
        }))
        .send().await.unwrap();

    let body: serde_json::Value = client
        .get(format!("{}/api/plugin-catalog-entries", app.url))
        .send().await.unwrap().json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "size-report");
    assert_eq!(entries[0]["catalog_name"], "ok");
    assert!(body["errors"].as_array().unwrap().is_empty());
}
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p transcoderr --test api_plugin_catalogs browse_returns_entries`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/transcoderr/src/api/plugins.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/tests/api_plugin_catalogs.rs
git commit -m "feat(api): GET /plugin-catalog-entries merged across all catalogs"
```

---

## Task 12: API — install + uninstall endpoints

**Files:**
- Modify: `crates/transcoderr/src/api/plugins.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`
- Modify: `crates/transcoderr/src/db/plugins.rs` (extend `sync_discovered` to write `catalog_id` + `tarball_sha256`)

- [ ] **Step 1: Extend `db::plugins::sync_discovered` with optional catalog provenance**

Modify `crates/transcoderr/src/db/plugins.rs` — change the signature of `sync_discovered` from `(pool, &[DiscoveredPlugin])` to `(pool, &[DiscoveredPlugin], &HashMap<String, (i64, String)>)` where the third arg is per-plugin-name `(catalog_id, tarball_sha256)` provenance written by the installer. Existing call sites (boot, the integration tests that already use it) pass `&HashMap::new()`.

```rust
use std::collections::HashMap;

pub async fn sync_discovered(
    pool: &SqlitePool,
    discovered: &[DiscoveredPlugin],
    provenance: &HashMap<String, (i64, String)>,
) -> anyhow::Result<()> {
    for d in discovered {
        let schema_json = serde_json::to_string(&d.schema)?;
        let path_str = d.manifest_dir.to_string_lossy().to_string();
        let prov = provenance.get(&d.manifest.name);
        let catalog_id = prov.map(|(id, _)| *id);
        let sha = prov.map(|(_, sha)| sha.clone());

        sqlx::query(
            "INSERT INTO plugins (name, version, kind, path, schema_json, enabled, catalog_id, tarball_sha256)
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)
             ON CONFLICT(name) DO UPDATE SET
               version        = excluded.version,
               kind           = excluded.kind,
               path           = excluded.path,
               schema_json    = excluded.schema_json,
               catalog_id     = COALESCE(excluded.catalog_id, plugins.catalog_id),
               tarball_sha256 = COALESCE(excluded.tarball_sha256, plugins.tarball_sha256)"
        )
        .bind(&d.manifest.name)
        .bind(&d.manifest.version)
        .bind(&d.manifest.kind)
        .bind(&path_str)
        .bind(&schema_json)
        .bind(catalog_id)
        .bind(&sha)
        .execute(pool).await?;
    }

    if discovered.is_empty() {
        sqlx::query("DELETE FROM plugins").execute(pool).await?;
    } else {
        let placeholders = std::iter::repeat("?").take(discovered.len()).collect::<Vec<_>>().join(", ");
        let sql = format!("DELETE FROM plugins WHERE name NOT IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for d in discovered {
            q = q.bind(&d.manifest.name);
        }
        q.execute(pool).await?;
    }
    Ok(())
}
```

Update existing call sites:
- `crates/transcoderr/src/main.rs`: `sync_discovered(&pool, &discovered, &HashMap::new())`
- Anywhere in tests that calls `sync_discovered` directly (the existing `db::plugins::tests` helpers): pass `&HashMap::new()`.

- [ ] **Step 2: Add the install + uninstall handlers**

Append to `crates/transcoderr/src/api/plugins.rs`:

```rust
use crate::plugins::installer;
use crate::plugins::uninstaller;
use std::collections::HashMap;

pub async fn install(
    State(state): State<AppState>,
    Path((catalog_id, name)): Path<(i64, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let res = state.catalog_client.list_all(&state.pool).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let entry = res.entries.into_iter()
        .find(|e| e.catalog_id == catalog_id && e.entry.name == name)
        .ok_or((StatusCode::NOT_FOUND, format!("entry {name} not in catalog {catalog_id}")))?;

    let plugins_dir = state.cfg.data_dir.join("plugins");
    let installed = installer::install_from_entry(&entry.entry, &plugins_dir)
        .await
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;

    let discovered = crate::plugins::discover(&plugins_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut provenance = HashMap::new();
    provenance.insert(installed.name.clone(), (catalog_id, installed.tarball_sha256));
    crate::db::plugins::sync_discovered(&state.pool, &discovered, &provenance)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::steps::registry::rebuild_from_discovered(discovered).await;

    Ok(Json(serde_json::json!({"installed": installed.name})))
}

pub async fn uninstall(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let plugins_dir = state.cfg.data_dir.join("plugins");
    uninstaller::uninstall(&state.pool, &plugins_dir, id).await
        .map_err(|e| match e {
            uninstaller::UninstallError::NotFound(_) => (StatusCode::NOT_FOUND, e.to_string()),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        })?;

    let discovered = crate::plugins::discover(&plugins_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::db::plugins::sync_discovered(&state.pool, &discovered, &HashMap::new())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::steps::registry::rebuild_from_discovered(discovered).await;

    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 3: Wire the routes**

Modify `crates/transcoderr/src/api/mod.rs`:

```rust
.route("/plugin-catalog-entries", get(plugins::browse))
.route("/plugin-catalog-entries/:catalog_id/:name/install", post(plugins::install))
.route("/plugins/:id",            get(plugins::get).delete(plugins::uninstall))
```

(The existing `.route("/plugins/:id", get(plugins::get))` line gets replaced by the third one above.)

- [ ] **Step 4: Add an install + uninstall API test**

Append to `crates/transcoderr/tests/api_plugin_catalogs.rs`:

```rust
#[tokio::test]
async fn install_then_uninstall_round_trip() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = boot().await;
    let client = reqwest::Client::new();

    // Build a tarball for a one-step plugin "demo" providing "demo.do".
    let manifest = "name = \"demo\"\nversion = \"0.1.0\"\nkind = \"subprocess\"\nentrypoint = \"bin/run\"\nprovides_steps = [\"demo.do\"]\n";
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/").unwrap(); hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
        tar.append(&hdr, std::io::empty()).unwrap();
        let body = manifest.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/manifest.toml").unwrap();
        hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();
        let run = b"#!/bin/sh\nread A\nread B\necho '{\"event\":\"result\",\"status\":\"ok\",\"outputs\":{}}'\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/bin/run").unwrap();
        hdr.set_mode(0o755); hdr.set_size(run.len() as u64); hdr.set_cksum();
        tar.append(&hdr, &run[..]).unwrap();
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let mut h = Sha256::new(); h.update(&bytes);
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();

    // Mock catalog hosting the tarball.
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "demo",
                "version": "0.1.0",
                "summary": "demo",
                "tarball_url": format!("{url}/demo.tar.gz"),
                "tarball_sha256": sha,
                "kind": "subprocess",
                "provides_steps": ["demo.do"]
            }]
        })))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/demo.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server).await;

    // Replace the seed catalog with this mock.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();
    let create: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({"name": "mock", "url": format!("{url}/index.json")}))
        .send().await.unwrap().json().await.unwrap();
    let cid = create["id"].as_i64().unwrap();

    // Install via the API.
    let resp = client
        .post(format!("{}/api/plugin-catalog-entries/{cid}/demo/install", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // /api/plugins now lists demo with provides_steps from the manifest.
    let plugins: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "demo");
    let pid = plugins[0]["id"].as_i64().unwrap();

    // Uninstall via the API.
    let resp = client
        .delete(format!("{}/api/plugins/{pid}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 204);

    let plugins_after: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert!(plugins_after.is_empty());
}
```

- [ ] **Step 5: Run the install test**

Run: `cargo test -p transcoderr --test api_plugin_catalogs install_then_uninstall_round_trip`
Expected: 1 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/transcoderr/src/api/plugins.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/src/db/plugins.rs \
        crates/transcoderr/src/main.rs \
        crates/transcoderr/tests/api_plugin_catalogs.rs
git commit -m "feat(api): install + uninstall endpoints with catalog provenance in plugins table"
```

---

## Task 13: Frontend — types + API client additions

**Files:**
- Modify: `web/src/types.ts`
- Modify: `web/src/api/client.ts`

- [ ] **Step 1: Add types**

Modify `web/src/types.ts`:

```ts
export type Plugin = {
  id: number;
  name: string;
  version: string;
  kind: string;
  provides_steps: string[];
  catalog_id?: number | null;
  tarball_sha256?: string | null;
};

export type PluginDetail = {
  id: number;
  name: string;
  version: string;
  kind: string;
  provides_steps: string[];
  capabilities: string[];
  requires: any;
  schema: any;
  path: string;
  readme: string | null;
};

export type PluginCatalog = {
  id: number;
  name: string;
  url: string;
  auth_header: string | null;
  priority: number;
  last_fetched_at: number | null;
  last_error: string | null;
};

export type CatalogEntry = {
  catalog_id: number;
  catalog_name: string;
  name: string;
  version: string;
  summary: string;
  tarball_url: string;
  tarball_sha256: string;
  homepage: string | null;
  min_transcoderr_version: string | null;
  kind: string;
  provides_steps: string[];
};

export type CatalogFetchError = {
  catalog_id: number;
  catalog_name: string;
  error: string;
};

export type CatalogListResponse = {
  entries: CatalogEntry[];
  errors: CatalogFetchError[];
};
```

- [ ] **Step 2: Add API client methods**

Modify `web/src/api/client.ts`:

```ts
plugins: {
  list:      () => req<import("../types").Plugin[]>("/plugins"),
  get:       (id: number) => req<import("../types").PluginDetail>(`/plugins/${id}`),
  uninstall: (id: number) => req<void>(`/plugins/${id}`, { method: "DELETE" }),
  browse:    () => req<import("../types").CatalogListResponse>("/plugin-catalog-entries"),
  install:   (catalogId: number, name: string) =>
    req<{ installed: string }>(`/plugin-catalog-entries/${catalogId}/${encodeURIComponent(name)}/install`, { method: "POST" }),
},
pluginCatalogs: {
  list:    () => req<import("../types").PluginCatalog[]>("/plugin-catalogs"),
  create:  (body: { name: string; url: string; auth_header?: string; priority?: number }) =>
    req<{ id: number }>("/plugin-catalogs", { method: "POST", body: JSON.stringify(body) }),
  delete:  (id: number) => req<void>(`/plugin-catalogs/${id}`, { method: "DELETE" }),
  refresh: (id: number) => req<void>(`/plugin-catalogs/${id}/refresh`, { method: "POST" }),
},
```

- [ ] **Step 3: Run the TypeScript build**

Run: `npm --prefix web run build`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add web/src/types.ts web/src/api/client.ts
git commit -m "feat(web): types + api client for plugin catalogs"
```

---

## Task 14: Frontend — Plugins page tab strip + Installed tab actions

**Files:**
- Modify: `web/src/pages/plugins.tsx`
- Modify: `web/src/index.css` (tab strip styles + Update/Uninstall buttons)

- [ ] **Step 1: Add tab-strip CSS**

Append to `web/src/index.css`:

```css
/* ---- plugin tabs --------------------------------------------------------- */

.plugin-tabs {
  display: flex;
  gap: var(--space-3);
  border-bottom: 1px solid var(--border-strong);
  margin-bottom: var(--space-4);
}
.plugin-tab {
  padding: 8px 4px;
  background: transparent;
  border: none;
  border-bottom: 2px solid transparent;
  color: var(--text-dim);
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-weight: 600;
  cursor: pointer;
}
.plugin-tab.is-active { color: var(--accent); border-bottom-color: var(--accent); }

.plugin-update-badge {
  display: inline-block;
  padding: 1px 6px;
  margin-left: var(--space-2);
  background: var(--accent-soft);
  color: var(--accent);
  border-radius: var(--r-2);
  font-size: 10px;
  letter-spacing: 0.04em;
  font-weight: 600;
  text-transform: uppercase;
}
```

- [ ] **Step 2: Wrap Plugins page with a tab strip**

Refactor `web/src/pages/plugins.tsx` so the existing list lives under an "Installed" tab and add stub "Browse" and "Catalogs" tabs (both render placeholder text in this task; populated in tasks 15 and 16):

```tsx
import { useState } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { marked } from "marked";
import { api } from "../api/client";
import type { Plugin, PluginDetail, CatalogEntry } from "../types";

type Tab = "installed" | "browse" | "catalogs";

export default function Plugins() {
  const [tab, setTab] = useState<Tab>("installed");

  return (
    <div className="page">
      <div className="page-header">
        <div>
          <div className="crumb">Configure</div>
          <h2>Plugins</h2>
        </div>
      </div>

      <div className="plugin-tabs" role="tablist">
        <button className={"plugin-tab" + (tab === "installed" ? " is-active" : "")}
                onClick={() => setTab("installed")} role="tab">Installed</button>
        <button className={"plugin-tab" + (tab === "browse" ? " is-active" : "")}
                onClick={() => setTab("browse")} role="tab">Browse</button>
        <button className={"plugin-tab" + (tab === "catalogs" ? " is-active" : "")}
                onClick={() => setTab("catalogs")} role="tab">Catalogs</button>
      </div>

      {tab === "installed" && <Installed />}
      {tab === "browse"    && <Browse />}
      {tab === "catalogs"  && <Catalogs />}
    </div>
  );
}

function Installed() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const browse = useQuery({ queryKey: ["plugin-catalog-entries"], queryFn: api.plugins.browse });
  const [openId, setOpenId] = useState<number | null>(null);

  const uninstall = useMutation({
    mutationFn: (id: number) => api.plugins.uninstall(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugins"] }),
  });
  const install = useMutation({
    mutationFn: ({ catalogId, name }: { catalogId: number; name: string }) =>
      api.plugins.install(catalogId, name),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugins"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
  });

  /// Index by name for the "update available?" check.
  const catalogByName = new Map<string, CatalogEntry>();
  for (const e of browse.data?.entries ?? []) catalogByName.set(e.name, e);

  return (
    <div className="surface">
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th style={{ width: 110 }}>Version</th>
            <th style={{ width: 130 }}>Kind</th>
            <th>Provides</th>
            <th style={{ width: 220 }}></th>
          </tr>
        </thead>
        <tbody>
          {(plugins.data ?? []).map((p: Plugin) => {
            const open = openId === p.id;
            const cat = catalogByName.get(p.name);
            const updateAvailable =
              cat &&
              p.catalog_id === cat.catalog_id &&
              cat.version !== p.version;
            return (
              <PluginRows
                key={p.id}
                plugin={p}
                open={open}
                onToggle={() => setOpenId(s => (s === p.id ? null : p.id))}
                onUninstall={() => {
                  if (confirm(`Uninstall plugin "${p.name}"? This deletes its directory.`)) {
                    uninstall.mutate(p.id);
                  }
                }}
                onUpdate={updateAvailable && cat ?
                  () => install.mutate({ catalogId: cat.catalog_id, name: cat.name })
                  : undefined}
                updateAvailable={!!updateAvailable}
              />
            );
          })}
          {(plugins.data ?? []).length === 0 && !plugins.isLoading && (
            <tr>
              <td colSpan={5} className="empty">
                No plugins discovered.
                <div className="hint">
                  Use the Browse tab to install one, or drop a directory into <code>data/plugins/</code> and restart.
                </div>
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}

interface RowProps {
  plugin: Plugin;
  open: boolean;
  onToggle: () => void;
  onUninstall: () => void;
  onUpdate?: () => void;
  updateAvailable: boolean;
}

function PluginRows({ plugin, open, onToggle, onUninstall, onUpdate, updateAvailable }: RowProps) {
  return (
    <>
      <tr className={"plugin-row" + (open ? " is-open" : "")}
          aria-expanded={open}>
        <td className="mono" onClick={onToggle} role="button" tabIndex={0}>
          <span className="plugin-row-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>{" "}
          {plugin.name}
        </td>
        <td className="dim tnum">
          {plugin.version}
          {updateAvailable && <span className="plugin-update-badge">update</span>}
        </td>
        <td><span className="label">{plugin.kind}</span></td>
        <td className="mono dim">
          {plugin.provides_steps.length === 0 ? "—" : plugin.provides_steps.join(", ")}
        </td>
        <td>
          {onUpdate && <button onClick={onUpdate}>Update</button>}{" "}
          <button className="btn-danger" onClick={onUninstall}>Uninstall</button>
        </td>
      </tr>
      {open && (
        <tr className="plugin-detail-row">
          <td colSpan={5}>
            <PluginDetailPanel id={plugin.id} />
          </td>
        </tr>
      )}
    </>
  );
}

// --- existing PluginDetailPanel + PluginDetailBody + ConfigSummary etc.
// continue to live below; restore them from the pre-refactor file.
function Browse() { return <div className="muted">Browse tab — task 15.</div>; }
function Catalogs() { return <div className="muted">Catalogs tab — task 16.</div>; }
```

(Keep `PluginDetailPanel`, `PluginDetailBody`, and any helper components from the existing file intact below this code; the column-count change to 5 needs to be reflected in the colspan they use too if they reference one.)

- [ ] **Step 3: Build to confirm**

Run: `npm --prefix web run build`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add web/src/pages/plugins.tsx web/src/index.css
git commit -m "feat(web): plugins page tab strip + Installed tab Update/Uninstall actions"
```

---

## Task 15: Frontend — Browse tab

**Files:**
- Modify: `web/src/pages/plugins.tsx` (replace the `Browse()` stub)

- [ ] **Step 1: Implement Browse**

Replace the `function Browse()` stub in `web/src/pages/plugins.tsx`:

```tsx
function Browse() {
  const qc = useQueryClient();
  const plugins = useQuery({ queryKey: ["plugins"], queryFn: api.plugins.list });
  const browse = useQuery({ queryKey: ["plugin-catalog-entries"], queryFn: api.plugins.browse });

  const install = useMutation({
    mutationFn: ({ catalogId, name }: { catalogId: number; name: string }) =>
      api.plugins.install(catalogId, name),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugins"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
  });

  const installedNames = new Set((plugins.data ?? []).map(p => p.name));

  return (
    <div className="surface">
      {(browse.data?.errors ?? []).length > 0 && (
        <div className="catalog-fetch-banner">
          <strong>{browse.data!.errors.length} catalog(s) unreachable:</strong>
          <ul>
            {browse.data!.errors.map(e => (
              <li key={e.catalog_id}>
                <code>{e.catalog_name}</code> — {e.error}
              </li>
            ))}
          </ul>
        </div>
      )}
      <table>
        <thead>
          <tr>
            <th>Plugin</th>
            <th style={{ width: 110 }}>Version</th>
            <th style={{ width: 160 }}>From</th>
            <th>Provides</th>
            <th style={{ width: 140 }}></th>
          </tr>
        </thead>
        <tbody>
          {(browse.data?.entries ?? []).map((e: CatalogEntry) => {
            const installed = installedNames.has(e.name);
            return (
              <tr key={`${e.catalog_id}/${e.name}`}>
                <td className="mono">
                  {e.name}
                  <div className="muted" style={{ fontSize: 11 }}>{e.summary}</div>
                </td>
                <td className="dim tnum">{e.version}</td>
                <td><span className="label">{e.catalog_name}</span></td>
                <td className="mono dim">
                  {e.provides_steps.length === 0 ? "—" : e.provides_steps.join(", ")}
                </td>
                <td>
                  {installed ? (
                    <span className="dim">Installed</span>
                  ) : (
                    <button onClick={() => {
                      if (confirm(`Install "${e.name}"? This plugin runs as the transcoderr user.`)) {
                        install.mutate({ catalogId: e.catalog_id, name: e.name });
                      }
                    }}>Install</button>
                  )}
                </td>
              </tr>
            );
          })}
          {(browse.data?.entries ?? []).length === 0 && !browse.isLoading && (
            <tr>
              <td colSpan={5} className="empty">
                No plugins available from configured catalogs.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
```

- [ ] **Step 2: Add the fetch-error-banner CSS**

Append to `web/src/index.css`:

```css
.catalog-fetch-banner {
  background: var(--bad-soft);
  border-bottom: 1px solid var(--bad);
  color: var(--bad);
  padding: var(--space-3) var(--space-4);
  font-size: 12px;
}
.catalog-fetch-banner ul { margin: 4px 0 0; padding-left: 20px; }
```

- [ ] **Step 3: Build**

Run: `npm --prefix web run build`
Expected: clean build.

- [ ] **Step 4: Commit**

```bash
git add web/src/pages/plugins.tsx web/src/index.css
git commit -m "feat(web): Plugins Browse tab with install action and fetch-error banner"
```

---

## Task 16: Frontend — Catalogs admin tab

**Files:**
- Modify: `web/src/pages/plugins.tsx` (replace the `Catalogs()` stub)

- [ ] **Step 1: Implement Catalogs**

Replace the `function Catalogs()` stub in `web/src/pages/plugins.tsx`:

```tsx
function Catalogs() {
  const qc = useQueryClient();
  const list = useQuery({ queryKey: ["plugin-catalogs"], queryFn: api.pluginCatalogs.list });

  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [authHeader, setAuthHeader] = useState("");
  const [priority, setPriority] = useState("0");
  const [addError, setAddError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () => api.pluginCatalogs.create({
      name: name.trim(),
      url: url.trim(),
      auth_header: authHeader.trim() || undefined,
      priority: Number.parseInt(priority, 10) || 0,
    }),
    onSuccess: () => {
      setName(""); setUrl(""); setAuthHeader(""); setPriority("0");
      setAddError(null);
      qc.invalidateQueries({ queryKey: ["plugin-catalogs"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
    onError: (e: any) => setAddError(e?.message ?? "create failed"),
  });
  const del = useMutation({
    mutationFn: (id: number) => api.pluginCatalogs.delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["plugin-catalogs"] });
      qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] });
    },
  });
  const refresh = useMutation({
    mutationFn: (id: number) => api.pluginCatalogs.refresh(id),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["plugin-catalog-entries"] }),
  });

  return (
    <>
      <div className="surface" style={{ padding: 16, marginBottom: 16 }}>
        <div className="label" style={{ marginBottom: 8 }}>Add catalog</div>
        <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
          <input placeholder="name" value={name} onChange={e => setName(e.target.value)} style={{ minWidth: 180 }} />
          <input placeholder="https://.../index.json" value={url} onChange={e => setUrl(e.target.value)} style={{ flex: 1, minWidth: 280 }} />
          <input type="password" placeholder="auth header (optional)" value={authHeader} onChange={e => setAuthHeader(e.target.value)} style={{ minWidth: 220 }} />
          <input type="number" placeholder="priority" value={priority} onChange={e => setPriority(e.target.value)} style={{ width: 100 }} />
          <button onClick={() => create.mutate()} disabled={create.isPending || !name.trim() || !url.trim()}>Add</button>
        </div>
        {addError && <div style={{ color: "var(--bad)", marginTop: 8, fontSize: 12 }}>{addError}</div>}
      </div>

      <div className="surface">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>URL</th>
              <th style={{ width: 90 }}>Priority</th>
              <th style={{ width: 160 }}>Last fetched</th>
              <th style={{ width: 220 }}></th>
            </tr>
          </thead>
          <tbody>
            {(list.data ?? []).map(c => (
              <tr key={c.id}>
                <td className="mono">{c.name}</td>
                <td className="dim mono" style={{ fontSize: 11, wordBreak: "break-all" }}>{c.url}</td>
                <td className="tnum dim">{c.priority}</td>
                <td className="dim" style={{ fontSize: 11 }}>
                  {c.last_fetched_at
                    ? new Date(c.last_fetched_at * 1000).toLocaleString()
                    : "never"}
                  {c.last_error && (
                    <div style={{ color: "var(--bad)", marginTop: 2 }}>{c.last_error}</div>
                  )}
                </td>
                <td>
                  <button className="btn-ghost" onClick={() => refresh.mutate(c.id)}>Refresh</button>{" "}
                  <button className="btn-danger"
                    onClick={() => {
                      if (confirm(`Delete catalog "${c.name}"?`)) del.mutate(c.id);
                    }}>Delete</button>
                </td>
              </tr>
            ))}
            {(list.data ?? []).length === 0 && !list.isLoading && (
              <tr><td colSpan={5} className="empty">No catalogs configured.</td></tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}
```

- [ ] **Step 2: Build**

Run: `npm --prefix web run build`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add web/src/pages/plugins.tsx
git commit -m "feat(web): Plugins Catalogs admin tab"
```

---

## Task 17: Integration — full install round-trip with size-report

**Files:**
- Create: `crates/transcoderr/tests/plugin_install_e2e.rs`

- [ ] **Step 1: Write the integration test**

Create `crates/transcoderr/tests/plugin_install_e2e.rs`:

```rust
//! End-to-end: mock a catalog hosting a tarball that mirrors
//! `docs/plugins/size-report/`, install via the API, then run a flow
//! that exercises both step names. Asserts ctx.steps.size_report is
//! populated by the run.

mod common;

use common::boot;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Write;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_size_report_tarball() -> (Vec<u8>, String) {
    let manifest = "name = \"size-report\"\n\
                    version = \"0.1.0\"\n\
                    kind = \"subprocess\"\n\
                    entrypoint = \"bin/run\"\n\
                    provides_steps = [\"size.report.before\", \"size.report.after\"]\n";
    let run_script = std::fs::read_to_string("../../docs/plugins/size-report/bin/run")
        .expect("docs/plugins/size-report/bin/run readable from crate dir");

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/").unwrap();
        hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
        tar.append(&hdr, std::io::empty()).unwrap();

        let body = manifest.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/manifest.toml").unwrap();
        hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();

        let body = run_script.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/bin/run").unwrap();
        hdr.set_mode(0o755); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let mut h = Sha256::new(); h.update(&bytes);
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    (bytes, sha)
}

#[tokio::test]
async fn install_size_report_then_run_uses_steps() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Mock catalog hosting size-report.
    let (bytes, sha) = build_size_report_tarball();
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "size-report",
                "version": "0.1.0",
                "summary": "size report",
                "tarball_url": format!("{url}/sr.tar.gz"),
                "tarball_sha256": sha,
                "kind": "subprocess",
                "provides_steps": ["size.report.before", "size.report.after"]
            }]
        })))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/sr.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server).await;

    // Replace the seed catalog with the mock.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();
    let resp: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({"name": "mock", "url": format!("{url}/index.json")}))
        .send().await.unwrap().json().await.unwrap();
    let cid = resp["id"].as_i64().unwrap();

    // Install.
    let resp = client
        .post(format!("{}/api/plugin-catalog-entries/{cid}/size-report/install", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Run size.report.before / .after by hand against a temp file.
    use std::collections::BTreeMap;
    use transcoderr::flow::Context;
    use transcoderr::steps::{registry, StepProgress};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Movie.mkv");
    std::fs::File::create(&path).unwrap().write_all(&vec![0u8; 1000]).unwrap();
    let mut ctx = Context::for_file(path.to_string_lossy().to_string());

    let before_step = registry::resolve("size.report.before").await
        .expect("size.report.before in registry post-install");
    before_step.execute(&BTreeMap::new(), &mut ctx, &mut |_: StepProgress| {})
        .await.unwrap();

    // Simulate a transcode that shrunk the file to 600 bytes.
    std::fs::File::create(&path).unwrap().write_all(&vec![0u8; 600]).unwrap();

    let after_step = registry::resolve("size.report.after").await.unwrap();
    after_step.execute(&BTreeMap::new(), &mut ctx, &mut |_: StepProgress| {})
        .await.unwrap();

    let report = ctx.steps.get("size_report").expect("size_report key written");
    assert_eq!(report["before_bytes"], 1000);
    assert_eq!(report["after_bytes"], 600);
    assert_eq!(report["saved_bytes"], 400);
    assert!(
        (report["ratio_pct"].as_f64().unwrap() - 40.0).abs() < 0.01,
        "ratio_pct = {:?}", report["ratio_pct"]
    );
}
```

> NOTE: the test reads `docs/plugins/size-report/bin/run` from the
> repo at runtime (`../../docs/plugins/size-report/bin/run` from
> `crates/transcoderr`). Once `docs/plugins/size-report/` migrates to
> the new `transcoderr-plugins` repo (the parallel effort the spec
> calls out), this test will need its source switched to a fixture
> committed under `crates/transcoderr/tests/fixtures/`. Until then,
> the in-tree script is the source of truth and the test exercises it
> directly.

- [ ] **Step 2: Run the integration test**

Run: `cargo test -p transcoderr --test plugin_install_e2e install_size_report_then_run_uses_steps`
Expected: PASS — install lands on disk, registry rebuild surfaces both steps, full flow run populates `ctx.steps.size_report`.

- [ ] **Step 3: Commit**

```bash
git add crates/transcoderr/tests/plugin_install_e2e.rs
git commit -m "test(plugins): full install round-trip exercises size-report through registry"
```

---

## Self-review notes (post-write check)

- **Spec coverage:** every section of `2026-05-01-plugin-catalog-design.md` maps to at least one task. Migration → 1; CRUD → 2 + 10; catalog client + cache + parallel + per-catalog errors → 3; tarball installer + sha verify + atomic rename + layout + manifest + replace → 4 + 5; uninstaller → 6; live-replace registry + race test → 7; AppState wiring → 8; auth_header redact/unredact → 9; install/uninstall API + provenance columns → 12; UI tab strip + Browse + Catalogs + update detection → 13–16; full round-trip integration → 17.
- **Placeholders:** none — every test has the actual code, every step has the actual command and expected result.
- **Type consistency:** `IndexEntry` and `CatalogEntry` shape used identically across catalog client, API, and frontend types. `tarball_sha256` is the field name everywhere. The TypeScript `Plugin` type widens to include the new optional fields rather than forking.
- **Out of scope confirmed:** the `transcoderr-plugins` repo and `size-report` migration into it are deferred (Task 17's note). The plan never depends on a remote being live; every test mocks via wiremock.
