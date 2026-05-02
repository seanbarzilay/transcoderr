//! REST endpoints for the workers registry. The WebSocket upgrade
//! handler (`/api/worker/connect`) lives in this same file (added in
//! Task 5) — one file per resource matches the existing
//! `api/sources.rs` / `api/notifiers.rs` pattern.

use crate::api::auth::AuthSource;
use crate::db;
use crate::http::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use rand::RngCore;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkerSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Redacted to "***" for token-authed callers; clear text for
    /// session callers (the UI). Local-worker rows always serialize as
    /// `null` since they have no token.
    pub secret_token: Option<String>,
    pub hw_caps: Option<serde_json::Value>,
    pub plugin_manifest: Option<serde_json::Value>,
    pub enabled: bool,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

fn row_to_summary(row: db::workers::WorkerRow, redact: bool) -> WorkerSummary {
    WorkerSummary {
        id: row.id,
        name: row.name,
        kind: row.kind,
        secret_token: row.secret_token.map(|t| if redact { "***".to_string() } else { t }),
        hw_caps: row
            .hw_caps_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        plugin_manifest: row
            .plugin_manifest_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        enabled: row.enabled != 0,
        last_seen_at: row.last_seen_at,
        created_at: row.created_at,
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<WorkerSummary>>, StatusCode> {
    let rows = db::workers::list_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let redact = auth == AuthSource::Token;
    Ok(Json(rows.into_iter().map(|r| row_to_summary(r, redact)).collect()))
}

#[derive(serde::Deserialize)]
pub struct CreateReq {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateResp {
    pub id: i64,
    /// One-time-display: this is the only response that ever contains
    /// the cleartext token. Subsequent reads return `***`.
    pub secret_token: String,
}

/// Mint a new remote worker. Returns `{id, secret_token}` once;
/// subsequent reads via `/api/workers` redact the token for token-authed
/// callers. The token is a 32-byte hex string (matches the format used
/// by the auto-provisioned *arr secret_tokens at `api/sources.rs`).
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let id = db::workers::insert_remote(&state.pool, &req.name, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "failed to insert worker row");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(CreateResp { id, secret_token: token }))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = db::workers::delete_remote(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
