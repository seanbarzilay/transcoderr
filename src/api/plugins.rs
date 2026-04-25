use crate::http::AppState;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Serialize)]
pub struct PluginRow {
    pub id: i64,
    pub name: String,
    pub version: String,
    pub kind: String,
    pub schema: serde_json::Value,
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct PatchPluginReq { pub enabled: bool }

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<PluginRow>>, StatusCode> {
    let rows = sqlx::query("SELECT id, name, version, kind, schema_json, enabled FROM plugins ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| {
        let schema_str: String = r.get(4);
        PluginRow {
            id: r.get(0),
            name: r.get(1),
            version: r.get(2),
            kind: r.get(3),
            schema: serde_json::from_str(&schema_str).unwrap_or_default(),
            enabled: r.get::<i64, _>(5) != 0,
        }
    }).collect();
    Ok(Json(out))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<PatchPluginReq>,
) -> Result<StatusCode, StatusCode> {
    let result = sqlx::query("UPDATE plugins SET enabled = ? WHERE id = ?")
        .bind(if req.enabled { 1i64 } else { 0i64 }).bind(id)
        .execute(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
