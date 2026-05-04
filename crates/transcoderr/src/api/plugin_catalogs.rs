use crate::api::auth::{redact_catalog_row, unredact_catalog_row, AuthSource};
use crate::db::plugin_catalogs;
use crate::http::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
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
    let rows = plugin_catalogs::list(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out: Vec<_> = rows
        .into_iter()
        .map(|r| {
            let mut v = json!({
                "id": r.id,
                "name": r.name,
                "url": r.url,
                "auth_header": r.auth_header,
                "priority": r.priority,
                "last_fetched_at": r.last_fetched_at,
                "last_error": r.last_error,
            });
            if auth == AuthSource::Token {
                redact_catalog_row(&mut v);
            }
            v
        })
        .collect();
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
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"id": id})))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = plugin_catalogs::delete(&state.pool, id)
        .await
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
