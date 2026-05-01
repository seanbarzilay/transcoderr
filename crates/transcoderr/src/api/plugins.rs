use crate::http::AppState;
use crate::plugins::catalog::ListAllResult;
use crate::plugins::installer;
use crate::plugins::manifest::Manifest;
use crate::plugins::uninstaller;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;
use std::path::PathBuf;

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

    let (provides_steps, capabilities, requires) = match manifest {
        Some(m) => (m.provides_steps, m.capabilities, m.requires),
        None => (vec![], vec![], serde_json::Value::Null),
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
        readme,
    }))
}

pub async fn browse(
    State(state): State<AppState>,
) -> Result<Json<ListAllResult>, StatusCode> {
    state.catalog_client
        .list_all(&state.pool)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// POST /api/plugin-catalog-entries/:catalog_id/:name/install
///
/// Resolves the catalog entry, downloads & verifies the tarball via the
/// installer, then re-discovers, syncs the DB with the catalog provenance
/// for this plugin, and rebuilds the in-memory step registry so the new
/// step set is live before the response returns.
pub async fn install(
    State(state): State<AppState>,
    Path((catalog_id, name)): Path<(i64, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.catalog_client.invalidate(catalog_id).await;
    let res = state
        .catalog_client
        .list_all(&state.pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let entry = res
        .entries
        .into_iter()
        .find(|e| e.catalog_id == catalog_id && e.entry.name == name)
        .ok_or((
            StatusCode::NOT_FOUND,
            format!("entry {name} not in catalog {catalog_id}"),
        ))?;

    let plugins_dir = state.cfg.data_dir.join("plugins");
    let installed = installer::install_from_entry(&entry.entry, &plugins_dir)
        .await
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;

    let discovered = crate::plugins::discover(&plugins_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut provenance = HashMap::new();
    provenance.insert(
        installed.name.clone(),
        (catalog_id, installed.tarball_sha256),
    );
    crate::db::plugins::sync_discovered(&state.pool, &discovered, &provenance)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::steps::registry::rebuild_from_discovered(discovered).await;

    Ok(Json(serde_json::json!({"installed": installed.name})))
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
