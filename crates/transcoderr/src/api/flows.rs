use crate::{
    db,
    flow::{parse_flow, validate::validate_flow_yaml},
    http::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use sqlx::Row;
use transcoderr_api_types::{CreateFlowReq, FlowDetail, FlowSummary, UpdateFlowReq};

#[derive(Serialize)]
pub struct ParseResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct FlowHealthRow {
    pub id: i64,
    pub ok: bool,
    pub issue_count: usize,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<FlowSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, name, enabled, version FROM flows ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows
        .into_iter()
        .map(|r| FlowSummary {
            id: r.get(0),
            name: r.get(1),
            enabled: r.get::<i64, _>(2) != 0,
            version: r.get(3),
        })
        .collect();
    Ok(Json(out))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<FlowDetail>, StatusCode> {
    let row = sqlx::query(
        "SELECT id, name, enabled, version, yaml_source, parsed_json FROM flows WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(FlowDetail {
        id: row.get(0),
        name: row.get(1),
        enabled: row.get::<i64, _>(2) != 0,
        version: row.get(3),
        yaml_source: row.get(4),
        parsed_json: serde_json::from_str(row.get::<&str, _>(5)).unwrap_or_default(),
    }))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateFlowReq>,
) -> Result<Json<FlowSummary>, StatusCode> {
    let parsed = parse_flow(&req.yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
    let id = db::flows::insert(&state.pool, &req.name, &req.yaml, &parsed)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(FlowSummary {
        id,
        name: req.name,
        enabled: true,
        version: 1,
    }))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateFlowReq>,
) -> Result<StatusCode, StatusCode> {
    let parsed = parse_flow(&req.yaml).map_err(|_| StatusCode::BAD_REQUEST)?;
    let parsed_json = serde_json::to_string(&parsed).unwrap();
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let cur: i64 = sqlx::query_scalar("SELECT version FROM flows WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let next = cur + 1;
    sqlx::query("UPDATE flows SET yaml_source = ?, parsed_json = ?, version = ?, updated_at = strftime('%s','now') WHERE id = ?")
        .bind(&req.yaml).bind(&parsed_json).bind(next).bind(id)
        .execute(&mut *tx).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    sqlx::query("INSERT INTO flow_versions (flow_id, version, yaml_source, created_at) VALUES (?, ?, ?, strftime('%s','now'))")
        .bind(id).bind(next).bind(&req.yaml)
        .execute(&mut *tx).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(en) = req.enabled {
        sqlx::query("UPDATE flows SET enabled = ? WHERE id = ?")
            .bind(if en { 1 } else { 0 })
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    tx.commit()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM flows WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn parse(Json(yaml): Json<String>) -> Json<ParseResult> {
    match parse_flow(&yaml) {
        Ok(f) => Json(ParseResult {
            ok: true,
            error: None,
            parsed: Some(serde_json::to_value(f).unwrap()),
        }),
        Err(e) => Json(ParseResult {
            ok: false,
            error: Some(e.to_string()),
            parsed: None,
        }),
    }
}

#[derive(serde::Deserialize)]
pub struct ValidateReq {
    pub yaml: String,
}

/// Static validation: surface YAML parse errors AND every CEL compile
/// error in `if:` conditionals and `{{ ... }}` templates. The runtime
/// `if:` evaluator silently treats compile/exec errors as `false`, so
/// without this endpoint a typo in a guard expression silently disables
/// the branch.
pub async fn validate(Json(req): Json<ValidateReq>) -> Json<serde_json::Value> {
    let report = validate_flow_yaml(&req.yaml);
    Json(serde_json::to_value(report).unwrap_or(serde_json::Value::Null))
}

/// Per-flow validation summary: one row per flow with `ok` + total
/// issue count (YAML parse errors and CEL/template compile errors all
/// count). Lets the flows-list page badge broken flows without N
/// round-trips to /flows/validate.
pub async fn health(State(state): State<AppState>) -> Result<Json<Vec<FlowHealthRow>>, StatusCode> {
    let rows = sqlx::query("SELECT id, yaml_source FROM flows ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows
        .into_iter()
        .map(|r| {
            let id: i64 = r.get(0);
            let yaml: &str = r.get(1);
            let report = validate_flow_yaml(yaml);
            FlowHealthRow {
                id,
                ok: report.ok,
                issue_count: report.issues.len(),
            }
        })
        .collect();
    Ok(Json(out))
}
