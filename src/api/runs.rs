use crate::http::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Serialize)]
pub struct RunSummary {
    pub id: i64,
    pub flow_id: i64,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Serialize)]
pub struct RunEvent {
    pub id: i64,
    pub job_id: i64,
    pub ts: i64,
    pub step_id: Option<String>,
    pub kind: String,
    pub payload: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct RunDetail {
    pub run: RunSummary,
    pub events: Vec<RunEvent>,
}

#[derive(Deserialize)]
pub struct ListParams {
    pub status: Option<String>,
    pub flow_id: Option<i64>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Deserialize)]
pub struct EventsParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct RerunResp {
    pub id: i64,
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Vec<RunSummary>>, StatusCode> {
    // Build query dynamically based on filters
    let limit = params.limit.unwrap_or(50).min(500);
    let offset = params.offset.unwrap_or(0);

    let rows = match (&params.status, &params.flow_id) {
        (Some(s), Some(fid)) => {
            sqlx::query("SELECT id, flow_id, status, created_at, finished_at FROM jobs WHERE status = ? AND flow_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?")
                .bind(s).bind(fid).bind(limit).bind(offset)
                .fetch_all(&state.pool).await
        }
        (Some(s), None) => {
            sqlx::query("SELECT id, flow_id, status, created_at, finished_at FROM jobs WHERE status = ? ORDER BY created_at DESC LIMIT ? OFFSET ?")
                .bind(s).bind(limit).bind(offset)
                .fetch_all(&state.pool).await
        }
        (None, Some(fid)) => {
            sqlx::query("SELECT id, flow_id, status, created_at, finished_at FROM jobs WHERE flow_id = ? ORDER BY created_at DESC LIMIT ? OFFSET ?")
                .bind(fid).bind(limit).bind(offset)
                .fetch_all(&state.pool).await
        }
        (None, None) => {
            sqlx::query("SELECT id, flow_id, status, created_at, finished_at FROM jobs ORDER BY created_at DESC LIMIT ? OFFSET ?")
                .bind(limit).bind(offset)
                .fetch_all(&state.pool).await
        }
    }
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let out = rows.into_iter().map(|r| RunSummary {
        id: r.get(0),
        flow_id: r.get(1),
        status: r.get(2),
        created_at: r.get(3),
        finished_at: r.get(4),
    }).collect();
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
) -> Result<Json<RunDetail>, StatusCode> {
    let row = sqlx::query("SELECT id, flow_id, status, created_at, finished_at FROM jobs WHERE id = ?")
        .bind(job_id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let run = RunSummary {
        id: row.get(0),
        flow_id: row.get(1),
        status: row.get(2),
        created_at: row.get(3),
        finished_at: row.get(4),
    };

    let event_rows = sqlx::query(
        "SELECT id, job_id, ts, step_id, kind, payload_json FROM run_events WHERE job_id = ? ORDER BY ts DESC LIMIT 200"
    )
    .bind(job_id).fetch_all(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let events = event_rows.into_iter().map(|r| {
        let payload_str: Option<String> = r.get(5);
        RunEvent {
            id: r.get(0),
            job_id: r.get(1),
            ts: r.get(2),
            step_id: r.get(3),
            kind: r.get(4),
            payload: payload_str.and_then(|s| serde_json::from_str(&s).ok()),
        }
    }).collect();

    Ok(Json(RunDetail { run, events }))
}

pub async fn events(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
    Query(params): Query<EventsParams>,
) -> Result<Json<Vec<RunEvent>>, StatusCode> {
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = params.offset.unwrap_or(0);

    // verify job exists
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM jobs WHERE id = ?")
        .bind(job_id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if exists.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let rows = sqlx::query(
        "SELECT id, job_id, ts, step_id, kind, payload_json FROM run_events WHERE job_id = ? ORDER BY ts ASC LIMIT ? OFFSET ?"
    )
    .bind(job_id).bind(limit).bind(offset)
    .fetch_all(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let out = rows.into_iter().map(|r| {
        let payload_str: Option<String> = r.get(5);
        RunEvent {
            id: r.get(0),
            job_id: r.get(1),
            ts: r.get(2),
            step_id: r.get(3),
            kind: r.get(4),
            payload: payload_str.and_then(|s| serde_json::from_str(&s).ok()),
        }
    }).collect();
    Ok(Json(out))
}

pub async fn cancel(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let result = sqlx::query(
        "UPDATE jobs SET status = 'cancelled', finished_at = strftime('%s','now') WHERE id = ? AND status IN ('running', 'pending')"
    )
    .bind(job_id).execute(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if result.rows_affected() == 0 {
        // Job not found or not in a cancellable state
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM jobs WHERE id = ?")
            .bind(job_id).fetch_optional(&state.pool).await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if exists.is_none() {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rerun(
    State(state): State<AppState>,
    Path(job_id): Path<i64>,
) -> Result<Json<RerunResp>, StatusCode> {
    let row = sqlx::query(
        "SELECT flow_id, flow_version, source_kind, file_path, trigger_payload_json FROM jobs WHERE id = ?"
    )
    .bind(job_id).fetch_optional(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let flow_id: i64 = row.get(0);
    let flow_version: i64 = row.get(1);
    let source_kind: String = row.get(2);
    let file_path: String = row.get(3);
    let payload: String = row.get(4);

    let now = crate::db::now_unix();
    let new_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO jobs (flow_id, flow_version, source_kind, file_path, trigger_payload_json, status, priority, attempt, created_at) \
         VALUES (?, ?, ?, ?, ?, 'pending', 0, 0, ?) RETURNING id"
    )
    .bind(flow_id).bind(flow_version).bind(&source_kind)
    .bind(&file_path).bind(&payload).bind(now)
    .fetch_one(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(RerunResp { id: new_id }))
}
