use crate::http::AppState;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use sqlx::Row;
use transcoderr_api_types::{CreatedIdResp as CreateResp, CreateSourceReq, SourceSummary, UpdateSourceReq};

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<SourceSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows.into_iter().map(|r| {
        let config_str: String = r.get(3);
        SourceSummary {
            id: r.get(0),
            kind: r.get(1),
            name: r.get(2),
            config: serde_json::from_str(&config_str).unwrap_or_default(),
            secret_token: r.get(4),
        }
    }).collect();
    Ok(Json(out))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateSourceReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    let config_json = serde_json::to_string(&req.config)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO sources (kind, name, config_json, secret_token) VALUES (?, ?, ?, ?) RETURNING id"
    )
    .bind(&req.kind).bind(&req.name).bind(&config_json).bind(&req.secret_token)
    .fetch_one(&state.pool).await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateResp { id }))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<SourceSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    Ok(Json(SourceSummary {
        id: row.get(0),
        kind: row.get(1),
        name: row.get(2),
        config: serde_json::from_str(&config_str).unwrap_or_default(),
        secret_token: row.get(4),
    }))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateSourceReq>,
) -> Result<StatusCode, StatusCode> {
    // Verify exists
    let row = sqlx::query("SELECT name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let name: String = req.name.unwrap_or_else(|| row.get(0));
    let config_json = match req.config {
        Some(c) => serde_json::to_string(&c).map_err(|_| StatusCode::BAD_REQUEST)?,
        None => row.get(1),
    };
    let secret_token: String = req.secret_token.unwrap_or_else(|| row.get(2));

    sqlx::query("UPDATE sources SET name = ?, config_json = ?, secret_token = ? WHERE id = ?")
        .bind(&name).bind(&config_json).bind(&secret_token).bind(id)
        .execute(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM sources WHERE id = ?")
        .bind(id).execute(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_fire(
    State(_state): State<AppState>,
    Path(_id): Path<i64>,
) -> StatusCode {
    // Stub: real SSE-driven test fire wired in Task 5
    StatusCode::NO_CONTENT
}
