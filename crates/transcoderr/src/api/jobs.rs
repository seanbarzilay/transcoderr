use crate::http::AppState;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use serde::Serialize;
use sqlx::Row;

#[derive(Serialize)]
pub struct JobRow {
    pub id: i64,
    pub flow_id: i64,
    pub flow_version: i64,
    pub source_kind: String,
    pub file_path: String,
    pub trigger_payload: serde_json::Value,
    pub status: String,
    pub status_label: Option<String>,
    pub priority: i64,
    pub current_step: Option<i64>,
    pub attempt: i64,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<JobRow>, StatusCode> {
    let row = sqlx::query(
        "SELECT id, flow_id, flow_version, source_kind, file_path, trigger_payload_json, \
         status, status_label, priority, current_step, attempt, created_at, started_at, finished_at \
         FROM jobs WHERE id = ?"
    )
    .bind(id).fetch_optional(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let payload_str: String = row.get(5);
    Ok(Json(JobRow {
        id: row.get(0),
        flow_id: row.get(1),
        flow_version: row.get(2),
        source_kind: row.get(3),
        file_path: row.get(4),
        trigger_payload: serde_json::from_str(&payload_str).unwrap_or_default(),
        status: row.get(6),
        status_label: row.get(7),
        priority: row.get(8),
        current_step: row.get(9),
        attempt: row.get(10),
        created_at: row.get(11),
        started_at: row.get(12),
        finished_at: row.get(13),
    }))
}
