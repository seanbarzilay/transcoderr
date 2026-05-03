# Distributed Transcoding — Piece 4 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Coordinator pushes plugin tarballs to connected workers so that each worker's `./plugins/` directory mirrors the coordinator's `db::plugins WHERE enabled=1`. After this piece plugins live on workers; Piece 5 wires plugin steps into the dispatch path.

**Architecture:** A new `GET /api/worker/plugins/:name/tarball` endpoint streams cached tarballs out of `<data_dir>/plugins/.tarball-cache/<name>-<sha>.tar.gz` (Bearer-on-Request auth). `register_ack.plugin_install` carries the full intended manifest. A new `PluginSync` WS envelope broadcasts the manifest on coordinator-side install/uninstall/toggle changes. Workers run a single `plugin_sync::sync` routine on `register_ack` AND on `PluginSync` — install missing, uninstall unwanted, then `registry::rebuild_from_discovered`.

**Tech Stack:** Rust 2021 (axum 0.7 + ws, sqlx + sqlite, tokio, anyhow, tracing, async_trait, reqwest, sha2). React 18 + TypeScript.

**Branch:** all tasks land on a fresh `feat/distributed-piece-4` branch off `main`. Implementer creates the branch before Task 1.

---

## File Structure

**New backend files:**
- `crates/transcoderr/src/api/worker_plugins.rs` — `GET /api/worker/plugins/:name/tarball` handler with Bearer-on-Request auth.
- `crates/transcoderr/src/worker/plugin_sync.rs` — worker-side sync routine + `compute_diff` helper + 5 unit tests.
- `crates/transcoderr/tests/plugin_push.rs` — 6-scenario integration suite.

**Modified backend files:**
- `crates/transcoderr/src/plugins/installer.rs` — `install_from_entry` gains `archive_to: Option<&Path>` + `auth_token: Option<&str>` parameters. Update 5 in-file tests.
- `crates/transcoderr/src/plugins/uninstaller.rs` — add `uninstall_by_name(plugins_dir, name)` worker-side helper; coordinator-side `uninstall` glob-deletes `.tarball-cache/<name>-*.tar.gz`.
- `crates/transcoderr/src/api/plugins.rs` — pass cache path to `install_from_entry`; broadcast `PluginSync` after install / uninstall / enable-toggle; capture old sha before install for stale-cache cleanup.
- `crates/transcoderr/src/api/workers.rs::handle_connection` — populate `register_ack.plugin_install` from `db::plugins::list_enabled`.
- `crates/transcoderr/src/api/mod.rs` — register `/api/worker/plugins/:name/tarball` in the **public** router.
- `crates/transcoderr/src/db/plugins.rs` — add `list_enabled(pool) -> Vec<PluginRow>` helper.
- `crates/transcoderr/src/worker/connections.rs` — add `broadcast_plugin_sync(manifest)` method.
- `crates/transcoderr/src/worker/protocol.rs` — `PluginSync` variant + struct + 1 round-trip test.
- `crates/transcoderr/src/worker/connection.rs` — receive loop dispatches `PluginSync`; trigger initial sync after register_ack.
- `crates/transcoderr/src/worker/daemon.rs` — pass `plugins_dir: PathBuf` and `coordinator_token: String` into the connection.
- `crates/transcoderr/src/worker/mod.rs` — `pub mod plugin_sync;`.

**No DB migration:** `db::plugins.tarball_sha256` already exists from PR #55 (plugin catalog). The cache directory is created on first install.

---

## Task 1: `installer::install_from_entry` adds `archive_to` + `auth_token` parameters

Mechanical signature change. Touches 5 in-file tests + 1 production call site (`api/plugins.rs:275`). Backwards-compatible default (None/None) gives byte-identical behavior. **Pause for user confirmation after this task.**

**Files:**
- Modify: `crates/transcoderr/src/plugins/installer.rs`
- Modify: `crates/transcoderr/src/api/plugins.rs` (one call site update)

- [ ] **Step 1: Branch verification**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Update `install_from_entry` signature + body**

In `crates/transcoderr/src/plugins/installer.rs`, replace the existing function (lines ~43-149) with:

```rust
/// Download, verify, extract, atomic-swap. Returns details of the
/// installed plugin on success. The caller is responsible for the
/// post-install bookkeeping (sync_discovered, registry rebuild).
///
/// Optional parameters:
/// - `archive_to`: when `Some`, the verified tarball is moved to that
///   path **before** staging cleanup. Coordinator passes the cache
///   path; worker passes `None`.
/// - `auth_token`: when `Some`, the GET request includes
///   `Authorization: Bearer <token>`. Worker passes its
///   `coordinator_token`; coordinator passes `None` (its catalog
///   fetches don't authenticate to itself).
pub async fn install_from_entry(
    entry: &IndexEntry,
    plugins_dir: &Path,
    archive_to: Option<&Path>,
    auth_token: Option<&str>,
) -> Result<InstalledPlugin, InstallError> {
    std::fs::create_dir_all(plugins_dir)?;
    let suffix: String = (0..8)
        .map(|_| (b'a' + (rand::random::<u8>() % 26)) as char)
        .collect();
    let staging = plugins_dir.join(format!(".tcr-install.{suffix}"));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut guard = StagingGuard(Some(staging.clone()));

    let tmp_tar = staging.join("plugin.tar.gz");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("reqwest client builds");
    let mut req = client.get(&entry.tarball_url);
    if let Some(token) = auth_token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(InstallError::Layout(format!("HTTP {}", resp.status())));
    }
    let body = resp.bytes().await?;
    let mut hasher = Sha256::new();
    hasher.update(&body);
    let got = hex(&hasher.finalize());
    if got != entry.tarball_sha256.to_lowercase() {
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
        return Err(InstallError::Layout(format!(
            "expected 1 top-level dir, got {}", top_dirs.len()
        )));
    }
    let top_dir = top_dirs.remove(0);
    let top_name = top_dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if top_name != entry.name {
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
        return Err(InstallError::Manifest(format!(
            "manifest.name is {:?}, expected {:?}", manifest.name, entry.name
        )));
    }

    // Atomic swap.
    let target = plugins_dir.join(&entry.name);
    let backup = plugins_dir.join(format!(".tcr-old.{}.{suffix}", entry.name));
    let backed_up = if target.exists() {
        std::fs::rename(&target, &backup)?;
        true
    } else {
        false
    };
    if let Err(e) = std::fs::rename(&top_dir, &target) {
        if backed_up {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(InstallError::Io(e));
    }
    let _ = std::fs::remove_dir_all(&backup);

    // Archive the verified tarball if requested. Done after atomic swap
    // so a failed install leaves no partial cache entry.
    if let Some(dest) = archive_to {
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Try rename (same volume); fall back to copy.
        if std::fs::rename(&tmp_tar, dest).is_err() {
            let _ = std::fs::copy(&tmp_tar, dest);
        }
    }

    guard.disarm();
    let _ = std::fs::remove_dir_all(&staging);

    Ok(InstalledPlugin {
        name: entry.name.clone(),
        plugin_dir: target,
        tarball_sha256: got,
    })
}
```

- [ ] **Step 3: Update the 5 in-file tests**

Find each of the 5 test call sites in the same file (around lines 246, 282, 314, 363, 394 — verify with `grep -n install_from_entry crates/transcoderr/src/plugins/installer.rs`) and add `, None, None` as the 3rd and 4th arguments. Example:

Before:
```rust
        let installed = install_from_entry(&entry, plugins_dir.path()).await.unwrap();
```

After:
```rust
        let installed = install_from_entry(&entry, plugins_dir.path(), None, None).await.unwrap();
```

Apply the same `, None, None` addition to all 5 in-file test call sites.

- [ ] **Step 4: Update the production call site**

In `crates/transcoderr/src/api/plugins.rs:275`, find:

```rust
        let installed = match installer::install_from_entry(&entry.entry, &plugins_dir).await {
```

Change to:

```rust
        let installed = match installer::install_from_entry(&entry.entry, &plugins_dir, None, None).await {
```

(Task 13 wires `archive_to=Some(...)` here. Task 1 keeps it None for now so this commit is a pure no-op refactor.)

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Run installer + plugin_install_e2e tests**

```bash
cargo test -p transcoderr --lib plugins::installer 2>&1 | grep -E "FAILED|^test result" | tail -5
cargo test -p transcoderr --test plugin_install_e2e 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED in either.

- [ ] **Step 7: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/plugins/installer.rs \
        crates/transcoderr/src/api/plugins.rs
git commit -m "feat(installer): install_from_entry accepts archive_to + auth_token"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 2: Uninstaller — best-effort tarball cache cleanup + worker-side `uninstall_by_name`

Two additions:
1. Coordinator-side `uninstall` glob-clears `.tarball-cache/<name>-*.tar.gz`.
2. New `uninstall_by_name(plugins_dir, name)` for the worker side (no DB; just removes the plugin directory).

**Files:**
- Modify: `crates/transcoderr/src/plugins/uninstaller.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add `uninstall_by_name` and cache cleanup**

In `crates/transcoderr/src/plugins/uninstaller.rs`, find the existing `pub async fn uninstall(...)` function. Replace it + append the new helper:

```rust
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

    // Best-effort: clear any cached source tarballs for this name.
    // Coordinator-side only — the worker has no cache dir, so the
    // glob simply finds nothing.
    clear_tarball_cache(plugins_dir, &name);

    Ok(name)
}

/// Worker-side uninstall: just remove the plugin directory. No DB,
/// no cache (workers don't keep tarballs). Best-effort — missing
/// directory is not an error.
pub fn uninstall_by_name(plugins_dir: &Path, name: &str) -> Result<(), UninstallError> {
    let dir = plugins_dir.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Best-effort glob deletion of `<plugins_dir>/.tarball-cache/<name>-*.tar.gz`.
/// Failures are logged but never returned — uninstall is already
/// well underway by the time we get here, and a stuck cache file
/// just wastes disk.
fn clear_tarball_cache(plugins_dir: &Path, name: &str) {
    let cache_dir = plugins_dir.join(".tarball-cache");
    let prefix = format!("{name}-");
    let suffix = ".tar.gz";
    let entries = match std::fs::read_dir(&cache_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let fname = entry.file_name();
        let fname = fname.to_string_lossy();
        if fname.starts_with(&prefix) && fname.ends_with(suffix) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!(error = ?e, file = %fname, "failed to remove cache file");
            }
        }
    }
}
```

- [ ] **Step 3: Add unit tests for `uninstall_by_name` and the cache helper**

Append inside the existing `#[cfg(test)] mod tests` block in `uninstaller.rs`:

```rust
    #[test]
    fn uninstall_by_name_removes_plugin_dir() {
        let dir = tempdir().unwrap();
        let plugins_dir = dir.path();
        let name = "foo";
        std::fs::create_dir_all(plugins_dir.join(name).join("bin")).unwrap();
        assert!(plugins_dir.join(name).exists());

        uninstall_by_name(plugins_dir, name).unwrap();
        assert!(!plugins_dir.join(name).exists());
    }

    #[test]
    fn uninstall_by_name_missing_dir_is_ok() {
        let dir = tempdir().unwrap();
        // No plugins/foo/ ever created — uninstall should succeed.
        uninstall_by_name(dir.path(), "foo").unwrap();
    }

    #[test]
    fn clear_tarball_cache_removes_matching_files() {
        let dir = tempdir().unwrap();
        let plugins_dir = dir.path();
        let cache = plugins_dir.join(".tarball-cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("foo-abc.tar.gz"), b"x").unwrap();
        std::fs::write(cache.join("foo-def.tar.gz"), b"y").unwrap();
        std::fs::write(cache.join("bar-xyz.tar.gz"), b"z").unwrap();

        clear_tarball_cache(plugins_dir, "foo");

        assert!(!cache.join("foo-abc.tar.gz").exists());
        assert!(!cache.join("foo-def.tar.gz").exists());
        assert!(cache.join("bar-xyz.tar.gz").exists(), "bar should be untouched");
    }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p transcoderr --lib plugins::uninstaller 2>&1 | tail -10
```

Expected: existing tests + 3 new ones all pass.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/plugins/uninstaller.rs
git commit -m "feat(uninstaller): add uninstall_by_name + cache cleanup"
```

---

## Task 3: `worker/protocol.rs` — `PluginSync` variant

Mechanical: one new message variant + struct + 1 round-trip test.

**Files:**
- Modify: `crates/transcoderr/src/worker/protocol.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Update the `Message` enum**

In `crates/transcoderr/src/worker/protocol.rs`, find the existing `pub enum Message { ... }` block (currently has 6 variants from Pieces 1+3). Add `PluginSync`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "type", content = "payload")]
pub enum Message {
    Register(Register),
    RegisterAck(RegisterAck),
    Heartbeat(Heartbeat),
    StepDispatch(StepDispatch),
    StepProgress(StepProgressMsg),
    StepComplete(StepComplete),
    PluginSync(PluginSync),
}
```

- [ ] **Step 3: Add the `PluginSync` struct**

After the existing `StepComplete` struct (or anywhere near the other message structs), append:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginSync {
    /// Full intended plugin manifest (NOT a delta). Workers run the
    /// same full-mirror sync logic on every receive.
    pub plugins: Vec<PluginInstall>,
}
```

`PluginInstall { name, version, sha256, tarball_url }` already exists from Piece 1.

- [ ] **Step 4: Add a round-trip test**

In the existing `mod tests` block, append:

```rust
    #[test]
    fn plugin_sync_round_trips() {
        let env = Envelope {
            id: "p1".into(),
            message: Message::PluginSync(PluginSync {
                plugins: vec![
                    PluginInstall {
                        name: "size-report".into(),
                        version: "0.1.2".into(),
                        sha256: "abc123".into(),
                        tarball_url: "https://coord/api/worker/plugins/size-report/tarball".into(),
                    },
                ],
            }),
        };
        assert_eq!(round_trip(&env), env);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains(r#""type":"plugin_sync""#), "snake_case tag: {s}");
    }
```

- [ ] **Step 5: Run protocol tests**

```bash
cargo test -p transcoderr --lib worker::protocol 2>&1 | tail -10
```

Expected: 8 passed (7 existing + 1 new).

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/protocol.rs
git commit -m "feat(worker): plugin_sync protocol variant"
```

---

## Task 4: `db::plugins::list_enabled` helper

Mechanical: a single new SELECT that the WS handler + broadcast helper both consume.

**Files:**
- Modify: `crates/transcoderr/src/db/plugins.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add `list_enabled` and a small `PluginRow` struct**

In `crates/transcoderr/src/db/plugins.rs`, append AFTER the existing `sync_discovered` function and BEFORE `mod tests`:

```rust
/// Subset of the `plugins` row needed by the worker manifest and
/// tarball serve endpoint. Fetched together so the wire envelope can
/// be built in one query.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PluginRow {
    pub name: String,
    pub version: String,
    pub tarball_sha256: Option<String>,
}

/// Enabled plugins, ordered by name. The result drives both
/// `register_ack.plugin_install` and the `PluginSync` broadcast
/// payload. Plugins missing a `tarball_sha256` (e.g. dev-loaded from
/// a path outside the catalog) are excluded — workers can't fetch
/// what isn't there.
pub async fn list_enabled(pool: &SqlitePool) -> anyhow::Result<Vec<PluginRow>> {
    Ok(sqlx::query_as(
        "SELECT name, version, tarball_sha256
           FROM plugins
          WHERE enabled = 1 AND tarball_sha256 IS NOT NULL
          ORDER BY name",
    )
    .fetch_all(pool)
    .await?)
}
```

- [ ] **Step 3: Add a unit test**

Append inside the existing `mod tests` block:

```rust
    #[tokio::test]
    async fn list_enabled_returns_enabled_only() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();

        // Seed two plugins via direct INSERT (avoid sync_discovered's
        // file-side dependencies).
        sqlx::query(
            "INSERT INTO plugins (name, version, kind, path, schema_json, enabled, tarball_sha256)
             VALUES ('a', '1.0', 'subprocess', '/x/a', '{}', 1, 'aaaa'),
                    ('b', '1.0', 'subprocess', '/x/b', '{}', 0, 'bbbb'),
                    ('c', '1.0', 'subprocess', '/x/c', '{}', 1, NULL)",
        )
        .execute(&pool).await.unwrap();

        let rows = list_enabled(&pool).await.unwrap();
        // Only 'a' qualifies: enabled=1 AND tarball_sha256 IS NOT NULL.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[0].tarball_sha256.as_deref(), Some("aaaa"));
    }
```

- [ ] **Step 4: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib db::plugins 2>&1 | tail -10
```

Expected: build clean; tests pass.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/db/plugins.rs
git commit -m "feat(db): plugins::list_enabled helper"
```

---

## Task 5: Tarball serve endpoint `/api/worker/plugins/:name/tarball`

New public-router handler. Bearer-on-Request auth. Streams the cached tarball file. **Pause for user confirmation after this task.**

**Files:**
- Create: `crates/transcoderr/src/api/worker_plugins.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create `crates/transcoderr/src/api/worker_plugins.rs`**

```rust
//! `/api/worker/plugins/:name/tarball` — coordinator serves the
//! cached source tarball to a connected worker. Auth is
//! Bearer-on-Request against `db::workers::secret_token` (same shape
//! as `/api/worker/connect`'s upgrade path).

use crate::db;
use crate::http::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use tokio_util::io::ReaderStream;

/// GET /api/worker/plugins/:name/tarball
///
/// Auth: `Authorization: Bearer <worker.secret_token>`. The worker's
/// `coordinator_token` from worker.toml goes here.
///
/// Responses:
/// - 200 + `application/x-gzip` body — the cached tarball
/// - 401 — missing/invalid Bearer
/// - 404 — plugin not found in `db::plugins WHERE enabled=1`, or the
///         cache file is missing on disk
/// - 500 — DB error
pub async fn tarball(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // 1. Bearer auth against workers.secret_token.
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();
    let _row = db::workers::get_by_token(&state.pool, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // 2. Lookup plugin row.
    let plugins = db::plugins::list_enabled(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let plugin = plugins
        .into_iter()
        .find(|p| p.name == name)
        .ok_or(StatusCode::NOT_FOUND)?;
    let sha = plugin.tarball_sha256.ok_or(StatusCode::NOT_FOUND)?;

    // 3. Open cache file.
    let cache_path = state
        .cfg
        .data_dir
        .join("plugins")
        .join(".tarball-cache")
        .join(format!("{name}-{sha}.tar.gz"));
    let file = match tokio::fs::File::open(&cache_path).await {
        Ok(f) => f,
        Err(_) => {
            tracing::warn!(?cache_path, "tarball cache miss");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // 4. Stream the file.
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-gzip")
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
```

- [ ] **Step 3: Verify `tokio-util` is a dep (it should be — used elsewhere)**

```bash
grep -nE "^tokio-util" crates/transcoderr/Cargo.toml
```

If absent, add to `[dependencies]`:
```toml
tokio-util = { version = "0.7", features = ["io"] }
```

- [ ] **Step 4: Register the route**

In `crates/transcoderr/src/api/mod.rs`:

1. Add `pub mod worker_plugins;` near the other `pub mod ...;` declarations.
2. Find the **public** Router block (the one with `/auth/login` and `/worker/connect`). Add:

```rust
        .route("/worker/plugins/:name/tarball", get(worker_plugins::tarball))
```

The endpoint goes in the **public** router — auth happens inside the handler.

- [ ] **Step 5: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean.

- [ ] **Step 6: Lib + Piece 1/2/3 integration tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test worker_connect --test local_worker --test remote_dispatch --test api_auth 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 7: Manual smoke (optional, end-to-end)**

```bash
mkdir -p /tmp/p4-test && cat > /tmp/p4-test/config.toml <<'EOF'
bind = "127.0.0.1:8082"
data_dir = "/tmp/p4-test"
[radarr]
bearer_token = "test"
EOF
mkdir -p /tmp/p4-test/plugins/.tarball-cache
echo "fake tarball" > /tmp/p4-test/plugins/.tarball-cache/dummy-deadbeef.tar.gz

./target/debug/transcoderr serve --config /tmp/p4-test/config.toml &
SERVER_PID=$!
sleep 2

# Mint a worker token.
TOK=$(curl -s -X POST http://127.0.0.1:8082/api/workers \
  -H "Content-Type: application/json" -d '{"name":"t"}' | grep -oE '"secret_token":"[^"]*"' | cut -d'"' -f4)

# Missing token → 401.
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:8082/api/worker/plugins/dummy/tarball
# expect: 401

# Wrong token → 401.
curl -s -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer wrong" \
  http://127.0.0.1:8082/api/worker/plugins/dummy/tarball
# expect: 401

# Valid token + missing plugin row → 404 (no DB row for "dummy").
curl -s -o /dev/null -w "%{http_code}\n" -H "Authorization: Bearer $TOK" \
  http://127.0.0.1:8082/api/worker/plugins/dummy/tarball
# expect: 404

kill $SERVER_PID
rm -rf /tmp/p4-test
```

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/worker_plugins.rs \
        crates/transcoderr/src/api/mod.rs \
        crates/transcoderr/Cargo.toml
git commit -m "feat(api): /api/worker/plugins/:name/tarball serve endpoint"
```

(If you didn't add tokio-util, drop Cargo.toml from the add list.)

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 6: `Connections::broadcast_plugin_sync` helper

Mechanical: one new method on the existing `Connections` struct from Piece 3.

**Files:**
- Modify: `crates/transcoderr/src/worker/connections.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add the broadcast method**

In `crates/transcoderr/src/worker/connections.rs`, find the existing `impl Connections { ... }` block. Append a new method:

```rust
    /// Broadcast a `PluginSync` envelope to every connected worker.
    /// Best-effort — dropped sends are logged but never block the
    /// caller. Caller is responsible for building the manifest.
    pub async fn broadcast_plugin_sync(
        &self,
        manifest: Vec<crate::worker::protocol::PluginInstall>,
    ) {
        use crate::worker::protocol::{Envelope, Message, PluginSync};
        let env = Envelope {
            id: format!("psync-{}", uuid::Uuid::new_v4()),
            message: Message::PluginSync(PluginSync { plugins: manifest }),
        };
        let map = self.senders.read().await;
        for (worker_id, tx) in map.iter() {
            if let Err(e) = tx.try_send(env.clone()) {
                tracing::warn!(worker_id, error = ?e, "plugin_sync broadcast: dropped");
            }
        }
    }
```

`try_send` (non-blocking) is correct here — if a worker's outbound channel is at capacity (32 from Piece 3), the broadcast logs and moves on rather than stalling the API request.

- [ ] **Step 3: Add a unit test**

In the existing `mod tests` block in `connections.rs`, append:

```rust
    #[tokio::test]
    async fn broadcast_plugin_sync_reaches_all_senders() {
        let conns = Connections::new();
        let (tx_a, mut rx_a) = mpsc::channel(4);
        let (tx_b, mut rx_b) = mpsc::channel(4);
        let _ga = conns.register_sender(1, tx_a).await;
        let _gb = conns.register_sender(2, tx_b).await;

        conns.broadcast_plugin_sync(vec![]).await;

        let env_a = rx_a.recv().await.expect("worker 1 got envelope");
        let env_b = rx_b.recv().await.expect("worker 2 got envelope");
        assert!(matches!(env_a.message, crate::worker::protocol::Message::PluginSync(_)));
        assert!(matches!(env_b.message, crate::worker::protocol::Message::PluginSync(_)));
    }
```

- [ ] **Step 4: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib worker::connections 2>&1 | tail -10
```

Expected: 5 passed (4 existing + 1 new).

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connections.rs
git commit -m "feat(worker): Connections::broadcast_plugin_sync"
```

---

## Task 7: `register_ack.plugin_install` populated with real manifest

Surgical edit to `api/workers.rs::handle_connection`. Currently `plugin_install: vec![]`.

**Files:**
- Modify: `crates/transcoderr/src/api/workers.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Build the manifest before sending register_ack**

In `crates/transcoderr/src/api/workers.rs`, find the existing `register_ack` construction inside `handle_connection`. It currently looks like:

```rust
    let ack = crate::worker::protocol::Envelope {
        id: correlation_id,
        message: crate::worker::protocol::Message::RegisterAck(crate::worker::protocol::RegisterAck {
            worker_id,
            plugin_install: vec![],
        }),
    };
```

Insert — RIGHT BEFORE the `let ack = ...` line — a manifest-build block that calls `db::plugins::list_enabled` and maps to `PluginInstall`:

```rust
    // Build the worker's intended plugin manifest from db::plugins.
    let manifest: Vec<crate::worker::protocol::PluginInstall> =
        match db::plugins::list_enabled(&state.pool).await {
            Ok(plugins) => plugins
                .into_iter()
                .filter_map(|p| {
                    let sha = p.tarball_sha256?;
                    Some(crate::worker::protocol::PluginInstall {
                        tarball_url: format!(
                            "{}/api/worker/plugins/{}/tarball",
                            state.public_url, p.name
                        ),
                        name: p.name,
                        version: p.version,
                        sha256: sha,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = ?e, "list_enabled failed; sending empty manifest");
                Vec::new()
            }
        };
```

Then update the `RegisterAck` construction to use it:

```rust
    let ack = crate::worker::protocol::Envelope {
        id: correlation_id,
        message: crate::worker::protocol::Message::RegisterAck(crate::worker::protocol::RegisterAck {
            worker_id,
            plugin_install: manifest,
        }),
    };
```

- [ ] **Step 3: Build + run integration tests (worker_connect MUST stay green)**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --test worker_connect 2>&1 | tail -10
```

Expected: build clean; 4 worker_connect tests pass.

The test `connect_with_valid_token_succeeds_and_register_persists` (Piece 1) doesn't assert on `plugin_install` content — it just checks the ack arrives and registers the row. Since fresh test DB has no plugins, the manifest is empty, matching the old `vec![]` behavior exactly.

- [ ] **Step 4: Lib tests + Piece 2/3 integration tests**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test local_worker --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -5
```

Expected: no FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/workers.rs
git commit -m "feat(api): register_ack.plugin_install carries real manifest"
```

---

## Task 8: `api/plugins.rs` install/uninstall/toggle handlers broadcast `PluginSync`

Three call sites in the existing handlers. Build the manifest after the DB write succeeds, then call `state.connections.broadcast_plugin_sync(manifest)`.

**Files:**
- Modify: `crates/transcoderr/src/api/plugins.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Identify the three call sites**

```bash
grep -nE "fn install|fn uninstall|fn set_enabled|sync_discovered|registry::rebuild" crates/transcoderr/src/api/plugins.rs | head -20
```

You should see at least:
- An install handler (around line 250-280) that calls `installer::install_from_entry`.
- An uninstall handler that calls `uninstaller::uninstall`.
- A `set_enabled` (or similar) handler for the enable toggle.

- [ ] **Step 3: Add a small `broadcast_manifest` helper at the bottom of the file**

Append (near the bottom, before any `#[cfg(test)]`):

```rust
/// Build the current plugin manifest and push a `PluginSync` to all
/// connected workers. Best-effort: errors are logged.
async fn broadcast_manifest(state: &AppState) {
    let plugins = match crate::db::plugins::list_enabled(&state.pool).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = ?e, "broadcast_manifest: list_enabled failed");
            return;
        }
    };
    let manifest: Vec<crate::worker::protocol::PluginInstall> = plugins
        .into_iter()
        .filter_map(|p| {
            let sha = p.tarball_sha256?;
            Some(crate::worker::protocol::PluginInstall {
                tarball_url: format!(
                    "{}/api/worker/plugins/{}/tarball",
                    state.public_url, p.name
                ),
                name: p.name,
                version: p.version,
                sha256: sha,
            })
        })
        .collect();
    state.connections.broadcast_plugin_sync(manifest).await;
}
```

(`AppState` is already in scope; if not, add `use crate::http::AppState;` at the top of the file.)

- [ ] **Step 4: Wire `broadcast_manifest` into the install handler**

Find the install handler. AFTER the existing post-install bookkeeping (`sync_discovered` + `rebuild_from_discovered` calls), before the function returns, add:

```rust
    broadcast_manifest(&state).await;
```

- [ ] **Step 5: Wire it into the uninstall handler**

Same shape: AFTER the existing uninstall bookkeeping (registry rebuild), add:

```rust
    broadcast_manifest(&state).await;
```

- [ ] **Step 6: Wire it into the enable-toggle handler**

If the file has an enable/disable handler that flips `plugins.enabled`, add the same `broadcast_manifest(&state).await;` after the DB write.

If there's no such handler in this file (toggling may live elsewhere — verify with `grep -rnE "UPDATE plugins SET enabled" crates/transcoderr/src/`), broadcast from wherever the enable toggle is implemented. If unfindable, leave a TODO comment in the broadcast helper noting that toggle propagation is a Piece-5 follow-up.

- [ ] **Step 7: Build + run integration tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
cargo test -p transcoderr --test plugin_install_e2e 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: clean build; no FAILED. The existing `plugin_install_e2e` test exercises the install handler with no connected workers, so `broadcast_manifest` runs and broadcasts to zero recipients (no-op).

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/plugins.rs
git commit -m "feat(api): install/uninstall/toggle broadcast plugin_sync"
```

---

## Task 9: `worker/plugin_sync.rs` — sync routine + `compute_diff` + 5 unit tests

The most complex new code path. Computes install/uninstall diff, runs both directions through the existing pipeline, then rebuilds the registry. **Pause for user confirmation after this task.**

**Files:**
- Create: `crates/transcoderr/src/worker/plugin_sync.rs`
- Modify: `crates/transcoderr/src/worker/mod.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Create `crates/transcoderr/src/worker/plugin_sync.rs`**

```rust
//! Worker-side plugin synchronisation. Called on `register_ack` and
//! on every `PluginSync` envelope. Mirrors the coordinator's manifest:
//! installs missing plugins, uninstalls anything the coordinator no
//! longer wants. After the sync, the worker's step registry is
//! rebuilt from the new on-disk discovery.

use crate::plugins::catalog::IndexEntry;
use crate::plugins::{installer, uninstaller};
use crate::worker::protocol::PluginInstall;
use std::path::{Path, PathBuf};

/// Output of `compute_diff`. Vectors are exclusive — anything in
/// `to_install` is not in `to_remove` and vice versa.
#[derive(Debug, PartialEq)]
pub struct Diff {
    /// Manifest entries that need to be installed (or replaced — a
    /// version bump shows up here too because the installer's
    /// atomic-swap path overwrites the existing dir).
    pub to_install: Vec<PluginInstall>,
    /// Plugin names currently installed but absent from the manifest.
    pub to_remove: Vec<String>,
}

/// Compute the install/remove plan from the current installed set
/// and the coordinator's intended manifest.
///
/// `installed` is `(name, sha256_or_none)`. `sha256_or_none == None`
/// means we don't know the local sha (e.g. a side-loaded plugin that
/// never went through `install_from_entry`); we treat such entries as
/// "needs replace" if the manifest mentions the name.
pub fn compute_diff(
    installed: &[(String, Option<String>)],
    manifest: &[PluginInstall],
) -> Diff {
    let manifest_names: std::collections::HashSet<&str> =
        manifest.iter().map(|m| m.name.as_str()).collect();

    let to_remove: Vec<String> = installed
        .iter()
        .filter(|(name, _)| !manifest_names.contains(name.as_str()))
        .map(|(name, _)| name.clone())
        .collect();

    let to_install: Vec<PluginInstall> = manifest
        .iter()
        .filter(|m| {
            // Already installed with matching sha → skip.
            !installed.iter().any(|(name, sha)| {
                name == &m.name && sha.as_deref() == Some(m.sha256.as_str())
            })
        })
        .cloned()
        .collect();

    Diff { to_install, to_remove }
}

/// Run a full mirror sync against the coordinator's manifest.
///
/// Best-effort: every install/uninstall is wrapped so a single
/// failure logs and continues with the rest. The caller cannot
/// distinguish "everything succeeded" from "something failed" —
/// failures land in the worker's logs, and the next `PluginSync`
/// or reconnect retries.
pub async fn sync(
    plugins_dir: &Path,
    manifest: Vec<PluginInstall>,
    coordinator_token: &str,
) {
    // 1. Discover currently-installed plugins.
    let installed = match crate::plugins::discover(plugins_dir) {
        Ok(d) => d
            .into_iter()
            .map(|p| {
                // Read the optional .tcr-sha256 marker file. We write
                // this on successful install (Step 3 below) so the
                // worker can answer "what sha was this installed
                // from?" without re-hashing.
                let sha_file = p.manifest_dir.join(".tcr-sha256");
                let sha = std::fs::read_to_string(&sha_file)
                    .ok()
                    .map(|s| s.trim().to_string());
                (p.manifest.name, sha)
            })
            .collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!(error = ?e, "plugin_sync: discover failed; treating as empty");
            Vec::new()
        }
    };

    // 2. Compute diff.
    let diff = compute_diff(&installed, &manifest);

    // 3. Uninstall what the coordinator doesn't want.
    for name in &diff.to_remove {
        match uninstaller::uninstall_by_name(plugins_dir, name) {
            Ok(_) => tracing::info!(name = %name, "plugin_sync: uninstalled"),
            Err(e) => tracing::warn!(name = %name, error = ?e, "plugin_sync: uninstall failed; skipping"),
        }
    }

    // 4. Install what's missing or version-bumped.
    for entry in &diff.to_install {
        let index_entry = IndexEntry {
            name: entry.name.clone(),
            version: entry.version.clone(),
            description: None,
            tarball_url: entry.tarball_url.clone(),
            tarball_sha256: entry.sha256.clone(),
            runtimes: Vec::new(),
        };
        match installer::install_from_entry(
            &index_entry,
            plugins_dir,
            None,
            Some(coordinator_token),
        )
        .await
        {
            Ok(installed) => {
                // Write the sha256 marker so the next sync's
                // `compute_diff` can no-op when the manifest hasn't
                // changed.
                let sha_path = installed.plugin_dir.join(".tcr-sha256");
                let _ = std::fs::write(&sha_path, &installed.tarball_sha256);
                tracing::info!(name = %entry.name, "plugin_sync: installed");
            }
            Err(e) => {
                tracing::warn!(name = %entry.name, error = ?e, "plugin_sync: install failed; skipping");
            }
        }
    }

    // 5. Rebuild the worker's step registry so newly-installed
    //    plugins' steps are resolvable. If discover fails here we
    //    can't rebuild — log and move on. Next sync retries.
    let discovered = match crate::plugins::discover(plugins_dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = ?e, "plugin_sync: post-sync discover failed; registry not rebuilt");
            return;
        }
    };
    crate::steps::registry::rebuild_from_discovered(discovered).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi(name: &str, sha: &str) -> PluginInstall {
        PluginInstall {
            name: name.into(),
            version: "1.0".into(),
            sha256: sha.into(),
            tarball_url: format!("http://x/{name}/tarball"),
        }
    }

    #[test]
    fn diff_empty_to_empty_is_empty() {
        let d = compute_diff(&[], &[]);
        assert_eq!(d, Diff { to_install: vec![], to_remove: vec![] });
    }

    #[test]
    fn diff_empty_installed_with_one_in_manifest_installs() {
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&[], &m);
        assert_eq!(d.to_install, m);
        assert!(d.to_remove.is_empty());
    }

    #[test]
    fn diff_matching_sha_is_noop() {
        let installed = vec![("a".into(), Some("aaa".into()))];
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&installed, &m);
        assert!(d.to_install.is_empty());
        assert!(d.to_remove.is_empty());
    }

    #[test]
    fn diff_version_bump_replaces() {
        let installed = vec![("a".into(), Some("aaa".into()))];
        let m = vec![pi("a", "bbb")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("a", "bbb")]);
        assert!(d.to_remove.is_empty(), "version bump uses install path's atomic swap, not remove+install");
    }

    #[test]
    fn diff_replaces_unknown_with_known() {
        let installed = vec![("x".into(), Some("xxx".into()))];
        let m = vec![pi("y", "yyy")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("y", "yyy")]);
        assert_eq!(d.to_remove, vec!["x".to_string()]);
    }

    #[test]
    fn diff_unknown_local_sha_is_treated_as_replace() {
        // Installed plugin has no .tcr-sha256 marker (side-loaded).
        // If manifest mentions it, we should reinstall to bring it
        // under management.
        let installed = vec![("a".into(), None)];
        let m = vec![pi("a", "aaa")];
        let d = compute_diff(&installed, &m);
        assert_eq!(d.to_install, vec![pi("a", "aaa")]);
        assert!(d.to_remove.is_empty());
    }
}
```

Note: this expects `IndexEntry` to have these fields: `name, version, description, tarball_url, tarball_sha256, runtimes`. If the actual struct in `crates/transcoderr/src/plugins/catalog.rs` has different fields, adapt the construction to match. Read the struct first:

```bash
grep -nE "pub struct IndexEntry|pub name|pub version|pub tarball" crates/transcoderr/src/plugins/catalog.rs | head -15
```

- [ ] **Step 3: Wire the module into `worker/mod.rs`**

Add `pub mod plugin_sync;` next to the other module declarations:

```rust
pub mod config;
pub mod connection;
pub mod connections;
pub mod daemon;
pub mod executor;
pub mod local;
pub mod plugin_sync;  // NEW
pub mod pool;
pub mod protocol;

pub use pool::*;
```

- [ ] **Step 4: Build + run unit tests**

```bash
cargo build -p transcoderr 2>&1 | tail -5
cargo test -p transcoderr --lib worker::plugin_sync 2>&1 | tail -10
```

Expected: 6 passed (5 listed + 1 bonus for unknown-local-sha case). If `IndexEntry` fields don't match, fix the mapping in `sync()` before re-running.

- [ ] **Step 5: Lib tests still green**

```bash
cargo test -p transcoderr --lib 2>&1 | grep -E "FAILED|^test result" | tail -3
```

Expected: no FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/plugin_sync.rs \
        crates/transcoderr/src/worker/mod.rs
git commit -m "feat(worker): plugin_sync — full-mirror diff + install/uninstall + registry rebuild"
```

**[PAUSE FOR USER CONFIRMATION HERE]**

---

## Task 10: `worker/connection.rs` receive loop dispatches `PluginSync` + initial sync after register_ack

The worker's WS receive loop already handles `step_dispatch` (Piece 3). Add a branch for `Message::PluginSync` and a single-slot queue so consecutive syncs serialize but don't block heartbeats.

**Files:**
- Modify: `crates/transcoderr/src/worker/connection.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Add a single-slot manifest queue + sync worker task**

The connection currently runs:
1. WS dial + register handshake.
2. Sender task (drains outbound mpsc → ws_sink).
3. Receive loop (matches inbound frames, including step_dispatch from Piece 3).

Add:
4. Sync worker task: holds an `Arc<tokio::sync::Mutex<Option<Vec<PluginInstall>>>>` plus a `Notify`. When the receive loop sees a `PluginSync`, it writes the manifest into the slot and notifies. The sync task wakes, takes the slot, and runs `plugin_sync::sync`. While it's running, additional manifests overwrite the slot (we only care about the latest).

Find the existing connection setup in `connect_once`. After the sender task spawn but before the heartbeat/receive loop, add:

```rust
    // Plugin-sync queue: single-slot. Latest manifest wins.
    let sync_slot: std::sync::Arc<tokio::sync::Mutex<Option<Vec<crate::worker::protocol::PluginInstall>>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let sync_notify = std::sync::Arc::new(tokio::sync::Notify::new());

    // Sync worker: drain the slot whenever notified, run plugin_sync::sync,
    // repeat. Lives for the connection's lifetime; aborted on disconnect.
    let sync_task = {
        let plugins_dir = ctx.plugins_dir.clone();
        let token = ctx.coordinator_token.clone();
        let slot = sync_slot.clone();
        let notify = sync_notify.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                let manifest = {
                    let mut g = slot.lock().await;
                    g.take()
                };
                if let Some(m) = manifest {
                    crate::worker::plugin_sync::sync(&plugins_dir, m, &token).await;
                }
            }
        })
    };
```

This requires `connect_once` to receive a `ctx: PluginSyncContext` (or similar) with `plugins_dir: PathBuf` and `coordinator_token: String`. Task 11 wires this through from `daemon.rs`.

For now, define a small struct in this file:

```rust
/// Context the worker connection needs for plugin sync. Threaded
/// from `daemon::run` → `connection::run` → `connect_once`.
#[derive(Clone)]
pub struct ConnectionContext {
    pub plugins_dir: std::path::PathBuf,
    pub coordinator_token: String,
}
```

And update `connect_once`'s signature to take a `&ConnectionContext`:

```rust
async fn connect_once<F>(
    url: &str,
    token: &str,
    build_register: &F,
    ctx: &ConnectionContext,
) -> anyhow::Result<()>
where
    F: Fn() -> Envelope,
{
    ...
}
```

Update `pub async fn run` similarly to take and pass through the context. The pre-existing `token` parameter equals `ctx.coordinator_token` — keep both for now to minimize churn (Task 11 cleans up the duplication if it bothers anyone).

- [ ] **Step 3: Trigger initial sync after register_ack**

In `connect_once`, after the existing `Message::RegisterAck(ack) => { ... tracing::info!("register acknowledged") ... }` arm, fire the initial sync if the manifest is non-empty:

```rust
    match ack.message {
        Message::RegisterAck(ack_payload) => {
            tracing::info!("worker register acknowledged");
            if !ack_payload.plugin_install.is_empty() {
                let mut g = sync_slot.lock().await;
                *g = Some(ack_payload.plugin_install);
                drop(g);
                sync_notify.notify_one();
            }
        }
        other => anyhow::bail!("expected register_ack, got {other:?}"),
    }
```

(Match the existing pattern — the variable name in your codebase may be `ack` rather than `ack_payload`. The point is to access the `plugin_install: Vec<PluginInstall>` field.)

- [ ] **Step 4: Add the `PluginSync` branch to the receive loop**

In the `tokio::select!` inside the heartbeat/receive loop, find the inbound-frame branch (`frame = ws_stream.next() => ...`). After the existing `Message::StepDispatch(...)` handling, add:

```rust
                    Message::PluginSync(p) => {
                        let mut g = sync_slot.lock().await;
                        *g = Some(p.plugins);
                        drop(g);
                        sync_notify.notify_one();
                    }
```

Place it inside the `match env.message { ... }` block so it sits alongside the existing variant arms.

- [ ] **Step 5: Abort the sync task on disconnect**

At every place the function returns (clean disconnect, error, etc.), add `sync_task.abort();` before the return — same shape as the existing `sender_task.abort()` calls.

- [ ] **Step 6: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean. Most likely failure: `connect_once`/`run` callers in `daemon.rs` haven't been updated yet — Task 11 fixes that. To smoke-test in isolation, you can pass a synthetic `ConnectionContext { plugins_dir: PathBuf::from("./plugins"), coordinator_token: token.clone() }` from `run` for now — Task 11 will pull it in from the WorkerConfig instead.

- [ ] **Step 7: Run worker_connect tests (regression net)**

```bash
cargo test -p transcoderr --test worker_connect 2>&1 | tail -10
```

Expected: 4 passed.

- [ ] **Step 8: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/connection.rs
git commit -m "feat(worker): connection dispatches PluginSync + triggers initial sync"
```

---

## Task 11: `worker/daemon.rs` threads `plugins_dir` + `coordinator_token` into the connection

Wires the `ConnectionContext` from Task 10 from `daemon::run`'s scope (where the WorkerConfig is loaded) down to `connection::run`.

**Files:**
- Modify: `crates/transcoderr/src/worker/daemon.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Build the `ConnectionContext` and pass it into `connection::run`**

Read the existing `daemon::run`:

```bash
sed -n '1,80p' crates/transcoderr/src/worker/daemon.rs
```

Find the call to `crate::worker::connection::run(...)` near the bottom of the function. Build the context just before, and pass it in:

```rust
    let ctx = crate::worker::connection::ConnectionContext {
        plugins_dir: std::path::PathBuf::from("./plugins"),
        coordinator_token: config.coordinator_token.clone(),
    };

    crate::worker::connection::run(
        config.coordinator_url,
        config.coordinator_token,
        build_register,
        ctx,
    )
    .await;
```

The plan-side note: `./plugins` matches the path the worker daemon's existing `plugins::discover(Path::new("./plugins"))` already uses for register payload construction. Keeping it consistent means `plugin_sync::sync` operates on the same directory the discover call reads from.

- [ ] **Step 3: Build smoke**

```bash
cargo build -p transcoderr 2>&1 | tail -5
```

Expected: clean. If the compiler complains about `connection::run`'s arity mismatch, walk back through Task 10's signature change.

- [ ] **Step 4: Worker_connect tests still green**

```bash
cargo test -p transcoderr --test worker_connect --test local_worker --test remote_dispatch 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 5: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/worker/daemon.rs
git commit -m "feat(worker): daemon threads plugins_dir + coordinator_token into connection"
```

---

## Task 12: `tests/plugin_push.rs` — 6 integration scenarios

End-to-end coverage for the tarball serve endpoint + register_ack manifest + live broadcast. Reuses Piece 3's WS connect harness.

**Files:**
- Create: `crates/transcoderr/tests/plugin_push.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Read the existing fake-worker harness in `tests/remote_dispatch.rs`**

```bash
cat crates/transcoderr/tests/remote_dispatch.rs | head -100
```

You'll see the canonical helpers: `mint_token`, `ws_connect`, `send_env`, `recv_env`, `fake_worker_register`. Copy the same patterns.

- [ ] **Step 3: Create the test file**

```rust
//! Integration tests for Piece 4's plugin push:
//!  1. tarball_endpoint_serves_cached_file
//!  2. tarball_endpoint_rejects_missing_token
//!  3. tarball_endpoint_404_for_unknown_plugin
//!  4. register_ack_carries_plugin_manifest
//!  5. plugin_install_broadcasts_plugin_sync
//!  6. plugin_uninstall_broadcasts_plugin_sync_without_it

mod common;

use common::boot;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use transcoderr::worker::protocol::{
    Envelope, Message, PluginManifestEntry, Register,
};

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn mint_token(client: &reqwest::Client, base: &str, name: &str) -> (i64, String) {
    let resp: serde_json::Value = client
        .post(format!("{base}/api/workers"))
        .json(&json!({"name": name}))
        .send().await.unwrap()
        .json().await.unwrap();
    (resp["id"].as_i64().unwrap(), resp["secret_token"].as_str().unwrap().to_string())
}

async fn ws_connect(base_ws: &str, token: &str) -> Ws {
    let mut req = format!("{base_ws}/api/worker/connect").as_str().into_client_request().unwrap();
    req.headers_mut().insert(AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws
}

async fn send_env(ws: &mut Ws, env: &Envelope) {
    let s = serde_json::to_string(env).unwrap();
    ws.send(WsMessage::Text(s)).await.unwrap();
}

async fn recv_env(ws: &mut Ws) -> Envelope {
    let raw = ws.next().await.unwrap().unwrap();
    match raw {
        WsMessage::Text(s) => serde_json::from_str(&s).unwrap(),
        other => panic!("expected text, got {other:?}"),
    }
}

async fn send_register_and_get_ack(ws: &mut Ws, name: &str) -> Envelope {
    let reg = Envelope {
        id: "reg-1".into(),
        message: Message::Register(Register {
            name: name.into(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: json!({}),
            available_steps: vec![],
            plugin_manifest: vec![PluginManifestEntry {
                name: "size-report".into(),
                version: "0.1.2".into(),
                sha256: None,
            }],
        }),
    };
    send_env(ws, &reg).await;
    recv_env(ws).await
}

/// Seed a plugin row + cache file directly via SQL/filesystem.
/// Bypasses the install handler so the test stays small. The
/// install-handler-driven path is exercised by tests 5 and 6.
async fn seed_plugin(app: &common::TestApp, name: &str, sha: &str, body: &[u8]) {
    sqlx::query(
        "INSERT INTO plugins (name, version, kind, path, schema_json, enabled, tarball_sha256)
         VALUES (?, '1.0', 'subprocess', ?, '{}', 1, ?)",
    )
    .bind(name)
    .bind(format!("{}/plugins/{name}", app.data_dir.display()))
    .bind(sha)
    .execute(&app.pool).await.unwrap();

    let cache = app.data_dir.join("plugins").join(".tarball-cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join(format!("{name}-{sha}.tar.gz")), body).unwrap();
}

async fn wait_for_plugin_sync(ws: &mut Ws, deadline: Duration) -> Option<Envelope> {
    let res = tokio::time::timeout(deadline, async {
        loop {
            let env = recv_env(ws).await;
            if matches!(env.message, Message::PluginSync(_)) {
                return env;
            }
        }
    }).await;
    res.ok()
}

#[tokio::test]
async fn tarball_endpoint_serves_cached_file() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "w1").await;

    let body: &[u8] = b"fake tarball body";
    let sha = "abc123def";
    seed_plugin(&app, "p1", sha, body).await;

    let resp = client
        .get(format!("{}/api/worker/plugins/p1/tarball", app.url))
        .bearer_auth(&token)
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(&bytes[..], body);
}

#[tokio::test]
async fn tarball_endpoint_rejects_missing_token() {
    let app = boot().await;
    seed_plugin(&app, "p1", "abc", b"x").await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/worker/plugins/p1/tarball", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn tarball_endpoint_404_for_unknown_plugin() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "w1").await;

    let resp = client
        .get(format!("{}/api/worker/plugins/nope/tarball", app.url))
        .bearer_auth(&token)
        .send().await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn register_ack_carries_plugin_manifest() {
    let app = boot().await;
    seed_plugin(&app, "size-report", "deadbeef", b"x").await;

    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;

    let ack = send_register_and_get_ack(&mut ws, "fake1").await;
    let plugin_install = match ack.message {
        Message::RegisterAck(a) => a.plugin_install,
        _ => panic!("expected register_ack"),
    };
    assert_eq!(plugin_install.len(), 1);
    assert_eq!(plugin_install[0].name, "size-report");
    assert_eq!(plugin_install[0].sha256, "deadbeef");
    assert!(plugin_install[0].tarball_url.contains("/api/worker/plugins/size-report/tarball"));
}

#[tokio::test]
async fn plugin_install_broadcasts_plugin_sync() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;
    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    let _ack = send_register_and_get_ack(&mut ws, "fake1").await;

    // Trigger an install by directly writing the row + invoking
    // broadcast_manifest. This bypasses the actual install handler
    // (which pulls a real catalog tarball) but exercises the
    // broadcast path that handler ends with.
    seed_plugin(&app, "extra", "xxx", b"x").await;
    transcoderr::api::plugins::broadcast_manifest_for_test(
        &transcoderr::http::AppState::clone_for_test(&app),
    ).await;

    let env = wait_for_plugin_sync(&mut ws, Duration::from_secs(2))
        .await
        .expect("worker should receive plugin_sync within 2s");
    let plugins = match env.message {
        Message::PluginSync(p) => p.plugins,
        _ => unreachable!(),
    };
    assert!(plugins.iter().any(|p| p.name == "extra"));
}

#[tokio::test]
async fn plugin_uninstall_broadcasts_plugin_sync_without_it() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let (_wid, token) = mint_token(&client, &app.url, "fake1").await;

    seed_plugin(&app, "going-away", "yyy", b"x").await;

    let base_ws = app.url.replace("http://", "ws://");
    let mut ws = ws_connect(&base_ws, &token).await;
    let _ack = send_register_and_get_ack(&mut ws, "fake1").await;

    // Remove the row + cache file directly, then re-broadcast.
    sqlx::query("DELETE FROM plugins WHERE name = 'going-away'")
        .execute(&app.pool).await.unwrap();
    let cache = app.data_dir.join("plugins").join(".tarball-cache");
    let _ = std::fs::remove_file(cache.join("going-away-yyy.tar.gz"));
    transcoderr::api::plugins::broadcast_manifest_for_test(
        &transcoderr::http::AppState::clone_for_test(&app),
    ).await;

    let env = wait_for_plugin_sync(&mut ws, Duration::from_secs(2))
        .await
        .expect("worker should receive plugin_sync within 2s");
    let plugins = match env.message {
        Message::PluginSync(p) => p.plugins,
        _ => unreachable!(),
    };
    assert!(plugins.iter().all(|p| p.name != "going-away"));
}
```

The two `_for_test` helpers are test-only shims you'll need to add:

In `crates/transcoderr/src/api/plugins.rs`, AFTER the existing `broadcast_manifest` helper, add:

```rust
/// Test-only re-export of `broadcast_manifest` so integration tests
/// can trigger the broadcast without going through the full install
/// handler (which needs a live catalog server).
#[doc(hidden)]
pub async fn broadcast_manifest_for_test(state: &crate::http::AppState) {
    broadcast_manifest(state).await;
}
```

In `crates/transcoderr/src/http.rs` (or wherever `AppState` is defined), AFTER the struct, add:

```rust
impl AppState {
    /// Test-only constructor that synthesizes an AppState from a
    /// TestApp. Used by integration tests that need to call
    /// AppState-consuming helpers without re-building the full
    /// fixture.
    #[doc(hidden)]
    pub fn clone_for_test(app: &impl AppStateLike) -> Self { app.as_app_state() }
}

#[doc(hidden)]
pub trait AppStateLike { fn as_app_state(&self) -> AppState; }
```

If that surface area feels heavy, an alternative is to make `AppState: Clone` (it already might be — verify) and have `TestApp` expose a `pub state: AppState` field. Adapt to whatever's cleanest in your codebase.

(If `TestApp` already exposes `state`, the simpler path is `transcoderr::api::plugins::broadcast_manifest_for_test(&app.state).await;` — read `tests/common/mod.rs`'s `TestApp` definition first to see what's exposed.)

- [ ] **Step 4: Run the new tests**

```bash
cargo test -p transcoderr --test plugin_push 2>&1 | tail -25
```

Expected: 6 passed. If something hangs, the most likely cause is the broadcast path not actually firing — verify that `broadcast_manifest_for_test` is reachable from the test binary.

- [ ] **Step 5: Run the full integration suite**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`. No FAILED.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/tests/plugin_push.rs \
        crates/transcoderr/src/api/plugins.rs \
        crates/transcoderr/src/http.rs
git commit -m "test(plugin_push): 6-scenario plugin push integration suite"
```

---

## Task 13: Stale tarball cache cleanup on version bump

When the install handler upgrades a plugin (same name, new sha), the OLD cache file would otherwise remain. Capture the old sha BEFORE install, delete the old cache file AFTER install succeeds.

**Files:**
- Modify: `crates/transcoderr/src/api/plugins.rs`

- [ ] **Step 1: Branch check**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
```

- [ ] **Step 2: Capture old sha + delete old cache file in the install handler**

In `crates/transcoderr/src/api/plugins.rs`, find the install handler (around the existing `installer::install_from_entry` call). Before the install runs:

```rust
    // Capture the previously-installed sha (if any) so we can clean
    // up the old cache file after a successful version bump.
    let old_sha: Option<String> = sqlx::query_scalar(
        "SELECT tarball_sha256 FROM plugins WHERE name = ?",
    )
    .bind(&entry.entry.name)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();
```

After the install + sync_discovered + registry rebuild succeed, before the existing `broadcast_manifest(&state).await;` line:

```rust
    // Stale-cache cleanup: if we just upgraded a plugin's version,
    // the old <name>-<old_sha>.tar.gz is left behind. Remove it.
    if let Some(old) = old_sha {
        if old != installed.tarball_sha256 {
            let old_path = state
                .cfg
                .data_dir
                .join("plugins")
                .join(".tarball-cache")
                .join(format!("{}-{}.tar.gz", entry.entry.name, old));
            let _ = std::fs::remove_file(&old_path);
        }
    }
```

(`installed` is the local variable holding the `InstalledPlugin` returned by `install_from_entry`. The variable name in your code may differ — adapt.)

- [ ] **Step 3: Wire `archive_to=Some(...)` in the same install handler**

While we're here, finally enable the cache write that Task 1 made optional. In the install handler, before the `installer::install_from_entry` call, build the cache path:

```rust
    let cache_path = state
        .cfg
        .data_dir
        .join("plugins")
        .join(".tarball-cache")
        .join(format!("{}-{}.tar.gz", entry.entry.name, entry.entry.tarball_sha256));
```

Update the `install_from_entry` call:

Before:
```rust
    let installed = match installer::install_from_entry(&entry.entry, &plugins_dir, None, None).await {
```

After:
```rust
    let installed = match installer::install_from_entry(
        &entry.entry,
        &plugins_dir,
        Some(&cache_path),
        None,
    ).await {
```

Note: this is the moment Piece 4's coordinator-side cache actually starts populating. Existing tests don't assert on cache file presence (they go through `install_from_entry` which now writes the cache), so they keep passing as long as the directory creation doesn't trip on permissions.

- [ ] **Step 4: Build + run tests**

```bash
cargo build -p transcoderr 2>&1 | tail -3
cargo test -p transcoderr --test plugin_install_e2e --test plugin_push 2>&1 | grep -E "FAILED|^test result" | tail -10
```

Expected: no FAILED.

- [ ] **Step 5: Run the full suite for confidence**

```bash
cargo test -p transcoderr 2>&1 | grep -E "FAILED|^test result" | tail -25
```

Expected: every line `test result: ok.`.

- [ ] **Step 6: Commit**

```bash
test "$(git branch --show-current)" = "feat/distributed-piece-4" || { echo "WRONG BRANCH"; exit 1; }
git add crates/transcoderr/src/api/plugins.rs
git commit -m "feat(api): coordinator caches tarballs + cleans up old shas on version bump"
```

---

## Self-Review Notes

This plan covers every section of the spec:

- **Sync semantics: full mirror** → Task 9 (compute_diff returns both `to_install` and `to_remove`).
- **Failure handling: best-effort skip + log** → Task 9 (`sync` wraps each install/uninstall in match arms that log warn and continue).
- **Live deltas: broadcast full manifest** → Task 6 (`broadcast_plugin_sync`) + Task 8 (call sites in install/uninstall handlers).
- **Tarball cache at `<data_dir>/plugins/.tarball-cache/<name>-<sha>.tar.gz`** → Task 13 (`archive_to=Some(cache_path)` in the install handler).
- **`GET /api/worker/plugins/:name/tarball` in PUBLIC router with Bearer-on-Request** → Task 5.
- **`register_ack.plugin_install` populated** → Task 7.
- **`installer::install_from_entry` gains `archive_to` + `auth_token`** → Task 1.
- **Uninstaller best-effort cache cleanup** → Task 2 (coordinator) + worker-side `uninstall_by_name` for plugin_sync's use.
- **Stale cache after version bump** → Task 13.
- **Worker spawns plugin sync as a separate task** → Task 10 (sync worker task + single-slot mutex).
- **Per-worker single-slot queue (latest manifest wins)** → Task 10.
- **6 integration scenarios** → Task 12.

Cross-task type/signature consistency:

- `installer::install_from_entry(entry, plugins_dir, archive_to, auth_token)` (Task 1) — called from `api/plugins.rs` (Task 13) with `Some(&cache_path), None`; called from `plugin_sync::sync` (Task 9) with `None, Some(token)`.
- `uninstaller::uninstall_by_name(plugins_dir, name)` (Task 2) — called from `plugin_sync::sync` (Task 9).
- `Message::PluginSync(PluginSync { plugins })` (Task 3) — broadcast by `Connections::broadcast_plugin_sync` (Task 6); received in `worker/connection.rs` receive loop (Task 10).
- `db::plugins::list_enabled(pool) -> Vec<PluginRow>` (Task 4) — called by `api/workers.rs` (Task 7) and `api/plugins.rs::broadcast_manifest` (Task 8).
- `Connections::broadcast_plugin_sync(manifest)` (Task 6) — called by `api/plugins.rs::broadcast_manifest` helper (Task 8).
- `ConnectionContext { plugins_dir, coordinator_token }` (Task 10) — built and passed in `daemon::run` (Task 11).
- `plugin_sync::sync(plugins_dir, manifest, coordinator_token)` (Task 9) — called from `worker/connection.rs` sync worker task (Task 10).

No placeholders. Every step has executable code or exact commands. All file paths absolute. Step granularity is bite-sized (each step is a 2-5 minute action). Frequent commits — 13 total commits, one per task.
