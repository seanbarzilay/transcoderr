use crate::http::AppState;
use crate::plugins::catalog::ListAllResult;
use crate::plugins::installer;
use crate::plugins::manifest::Manifest;
use crate::plugins::uninstaller;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

/// Summary row in the list view. Includes `provides_steps` so the page
/// can show the operator-facing step names without an extra round-trip,
/// and `catalog_id` so the Installed tab can detect "update available"
/// by joining against the merged catalog-entries list.
#[derive(Serialize)]
pub struct PluginRow {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub provides_steps: Vec<String>,
    /// Catalog this plugin was installed from. NULL when installed
    /// manually (operator dropped the directory into data/plugins/).
    pub catalog_id: Option<i64>,
    /// sha256 of the tarball at install time. NULL for manual installs.
    /// Surfaced for parity with `catalog_id`; not currently consumed by
    /// the UI but useful for future "drifted from catalog" detection.
    pub tarball_sha256: Option<String>,
}

/// Full detail for the expanded view. Includes the entire manifest plus
/// the contents of README.md from the plugin directory if one exists.
/// Markdown is returned raw -- the UI renders it.
#[derive(Serialize)]
pub struct PluginDetail {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub provides_steps: Vec<String>,
    pub capabilities: Vec<String>,
    pub requires: serde_json::Value,
    pub schema: serde_json::Value,
    /// Filesystem path of the plugin directory on the server. Useful as
    /// a "where to edit this" hint -- we render it in the detail view.
    pub path: String,
    /// One-line description from the manifest's `summary` field. `None`
    /// for older / hand-rolled plugins that don't set it.
    pub summary: Option<String>,
    /// Manifest's `min_transcoderr_version` field if set. Rendered as a
    /// "Min version v0.X.0" badge in the detail panel.
    pub min_transcoderr_version: Option<String>,
    /// Bare executable names the plugin shells out to. Rendered next to
    /// Capabilities so the operator sees what the plugin needs.
    pub runtimes: Vec<String>,
    /// Manifest's `deps` shell command if set (e.g. `pip install -r
    /// requirements.txt`). Rendered as a `<code>` block in the detail
    /// panel so the operator sees what's run on install / boot.
    pub deps: Option<String>,
    /// Verbatim README.md contents. `None` when the plugin doesn't ship one.
    pub readme: Option<String>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<PluginRow>>, StatusCode> {
    let rows = sqlx::query(
        "SELECT id, name, version, kind, path, catalog_id, tarball_sha256 \
         FROM plugins ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows
        .into_iter()
        .map(|r| {
            let path: Option<String> = r.get(4);
            let provides_steps = path
                .as_deref()
                .and_then(read_manifest)
                .map(|m| m.provides_steps)
                .unwrap_or_default();
            PluginRow {
                id: r.get(0),
                name: r.get(1),
                version: r.get(2),
                kind: r.get(3),
                provides_steps,
                catalog_id: r.get(5),
                tarball_sha256: r.get(6),
            }
        })
        .collect();
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PluginDetail>, StatusCode> {
    let row = sqlx::query("SELECT id, name, version, kind, path, schema_json FROM plugins WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let path_str: Option<String> = row.get(4);
    let path_str = path_str.unwrap_or_default();
    let manifest = read_manifest(&path_str);
    let readme = read_readme(&path_str);
    let schema_str: String = row.get(5);
    let schema: serde_json::Value = serde_json::from_str(&schema_str).unwrap_or_default();

    let (
        provides_steps,
        capabilities,
        requires,
        summary,
        min_transcoderr_version,
        runtimes,
        deps,
    ) = match manifest {
        Some(m) => (
            m.provides_steps,
            m.capabilities,
            m.requires,
            m.summary,
            m.min_transcoderr_version,
            m.runtimes,
            m.deps,
        ),
        None => (
            vec![],
            vec![],
            serde_json::Value::Null,
            None,
            None,
            vec![],
            None,
        ),
    };

    Ok(Json(PluginDetail {
        id: row.get(0),
        name: row.get(1),
        version: row.get(2),
        kind: row.get(3),
        provides_steps,
        capabilities,
        requires,
        schema,
        path: path_str,
        summary,
        min_transcoderr_version,
        runtimes,
        deps,
        readme,
    }))
}

pub async fn browse(
    State(state): State<AppState>,
) -> Result<Json<ListAllResult>, StatusCode> {
    let mut res = state
        .catalog_client
        .list_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Compute per-entry runtime availability so the FE can disable
    // Install up front instead of bouncing the operator on click.
    for e in &mut res.entries {
        e.missing_runtimes = state.runtime_checker.missing(&e.entry.runtimes).await;
    }
    Ok(Json(res))
}

/// POST /api/plugin-catalog-entries/:catalog_id/:name/install
///
/// Returns an SSE stream:
/// - `event: status`  data `{"message": "..."}`            — milestone log (extracting, sha verified, deps starting, etc.)
/// - `event: log`     data `{"stream": "stdout"|"stderr", "line": "..."}` — raw deps output, line at a time
/// - `event: done`    data `{"installed": "<name>"}`       — terminal success
/// - `event: error`   data `{"status": <http_status>, "message": "..."}` — terminal failure
///
/// Always responds 200 with `text/event-stream`; the actual outcome is in
/// the terminal event. The install task is `tokio::spawn`'d so it survives
/// client disconnect — pip keeps running, the DB sync still happens, the
/// step registry still rebuilds. The SSE stream just goes silent.
pub async fn install(
    State(state): State<AppState>,
    Path((catalog_id, name)): Path<(i64, String)>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    let pool = state.pool.clone();
    let plugins_dir = state.cfg.data_dir.join("plugins");
    let catalog_client = state.catalog_client.clone();
    let runtime_checker = state.runtime_checker.clone();
    let task_name = name.clone();
    // Cloned for the post-install `broadcast_manifest` call so the
    // spawned task owns its own handle on connections + public_url.
    let state_for_broadcast = state.clone();

    tokio::spawn(async move {
        // Helpers (closures): each emits one SSE Event. send returns Err
        // if the receiver was dropped (client disconnected); we ignore
        // that since the spawned task should still complete.
        let send = |ev: Event| {
            let _ = tx.send(ev);
        };
        let status = |msg: &str| {
            send(
                Event::default()
                    .event("status")
                    .data(serde_json::json!({"message": msg}).to_string()),
            );
        };
        let error = |code: StatusCode, msg: &str| {
            send(
                Event::default()
                    .event("error")
                    .data(
                        serde_json::json!({
                            "status": code.as_u16(),
                            "message": msg,
                        })
                        .to_string(),
                    ),
            );
        };

        tracing::info!(plugin = %task_name, catalog_id, "installing plugin");
        status(&format!("Looking up {task_name} in catalog {catalog_id}"));

        catalog_client.invalidate(catalog_id).await;
        let res = match catalog_client.list_all(&pool).await {
            Ok(r) => r,
            Err(e) => {
                error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                return;
            }
        };
        let entry = match res
            .entries
            .into_iter()
            .find(|e| e.catalog_id == catalog_id && e.entry.name == task_name)
        {
            Some(e) => e,
            None => {
                error(
                    StatusCode::NOT_FOUND,
                    &format!("entry {task_name} not in catalog {catalog_id}"),
                );
                return;
            }
        };

        let missing = runtime_checker.missing(&entry.entry.runtimes).await;
        if !missing.is_empty() {
            error(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!(
                    "missing runtime(s) on PATH: {} -- install them on the host first",
                    missing.join(", ")
                ),
            );
            return;
        }

        status("Downloading + verifying tarball");
        let installed = match installer::install_from_entry(&entry.entry, &plugins_dir, None, None).await {
            Ok(i) => i,
            Err(e) => {
                error(StatusCode::UNPROCESSABLE_ENTITY, &e.to_string());
                return;
            }
        };

        if let Some(deps_cmd) = read_manifest(&installed.plugin_dir.to_string_lossy())
            .and_then(|m| m.deps)
        {
            tracing::info!(plugin = %task_name, deps = %deps_cmd, "running plugin deps");
            status(&format!("Running deps: {deps_cmd}"));

            // Forward each line of pip stdout/stderr to the SSE stream as
            // a `log` event. The closure is called synchronously per
            // line by deps::run, so this is just a non-blocking send.
            let log_tx = tx.clone();
            let res = crate::plugins::deps::run(
                &installed.plugin_dir,
                &deps_cmd,
                |stream, line| {
                    let _ = log_tx.send(
                        Event::default()
                            .event("log")
                            .data(
                                serde_json::json!({
                                    "stream": match stream {
                                        crate::plugins::deps::Stream::Stdout => "stdout",
                                        crate::plugins::deps::Stream::Stderr => "stderr",
                                    },
                                    "line": line,
                                })
                                .to_string(),
                            ),
                    );
                },
            )
            .await;
            drop(log_tx);

            if let Err(e) = res {
                tracing::warn!(plugin = %task_name, error = %e, "plugin deps failed; rolling back install");
                let _ = std::fs::remove_dir_all(&installed.plugin_dir);
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("deps install failed: {e}"),
                );
                return;
            }
            status("Deps complete");
        }

        status("Registering plugin");
        let discovered = match crate::plugins::discover(&plugins_dir) {
            Ok(d) => d,
            Err(e) => {
                error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
                return;
            }
        };
        let mut provenance = HashMap::new();
        provenance.insert(
            installed.name.clone(),
            (catalog_id, installed.tarball_sha256),
        );
        if let Err(e) = crate::db::plugins::sync_discovered(&pool, &discovered, &provenance).await
        {
            error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            return;
        }
        crate::steps::registry::rebuild_from_discovered(discovered).await;

        broadcast_manifest(&state_for_broadcast).await;

        tracing::info!(plugin = %task_name, "plugin install complete");
        send(
            Event::default()
                .event("done")
                .data(serde_json::json!({"installed": installed.name}).to_string()),
        );
        // tx drops here; receiver sees end-of-stream and the SSE response
        // closes naturally.
    });

    Sse::new(UnboundedReceiverStream::new(rx).map(Ok))
        .keep_alive(KeepAlive::default())
}

/// DELETE /api/plugins/:id
///
/// Removes the on-disk plugin directory and DB row, then rediscovers the
/// remaining set, syncs the DB (which prunes the now-missing row if
/// uninstall raced anything), and rebuilds the step registry so the
/// removed plugin's steps are no longer dispatchable.
pub async fn uninstall(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, (StatusCode, String)> {
    let plugins_dir = state.cfg.data_dir.join("plugins");
    uninstaller::uninstall(&state.pool, &plugins_dir, id)
        .await
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

    broadcast_manifest(&state).await;

    Ok(StatusCode::NO_CONTENT)
}

/// Re-parse the on-disk manifest. Returns None if the directory is gone
/// or the file isn't valid TOML — we'd rather show partial data than
/// blow up the whole list.
fn read_manifest(dir: &str) -> Option<Manifest> {
    if dir.is_empty() {
        return None;
    }
    let mp = PathBuf::from(dir).join("manifest.toml");
    let raw = std::fs::read_to_string(&mp).ok()?;
    toml::from_str(&raw).ok()
}

/// Slurp README.md from the plugin directory. Cap the read at a sensible
/// size so a hostile or accidentally-huge file can't pin server memory.
fn read_readme(dir: &str) -> Option<String> {
    const MAX_BYTES: u64 = 256 * 1024;
    if dir.is_empty() {
        return None;
    }
    let p = PathBuf::from(dir).join("README.md");
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() > MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(&p).ok()
}

/// Build the current plugin manifest and push a `PluginSync` to all
/// connected workers. Best-effort: errors are logged.
///
/// TODO(piece-5): when an explicit enable/disable toggle handler lands
/// in this file, wire `broadcast_manifest` into it as well so workers
/// pick up toggle changes without an install/uninstall round-trip.
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

/// Test-only re-export of `broadcast_manifest` so integration tests
/// can trigger the broadcast without going through the full install
/// handler (which needs a live catalog server).
#[doc(hidden)]
pub async fn broadcast_manifest_for_test(state: &crate::http::AppState) {
    broadcast_manifest(state).await;
}
