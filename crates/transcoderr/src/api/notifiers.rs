use crate::api::auth::{redact_notifier_config, unredact_notifier_config, AuthSource};
use crate::{db, http::AppState, notifiers};
use axum::Extension;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use sqlx::Row;
use transcoderr_api_types::{CreatedIdResp as CreateResp, NotifierReq, NotifierSummary};

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<NotifierSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, name, kind, config_json FROM notifiers ORDER BY name")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let out = rows
        .into_iter()
        .map(|r| {
            let config_str: String = r.get(3);
            let mut config: serde_json::Value =
                serde_json::from_str(&config_str).unwrap_or_default();
            if auth == AuthSource::Token {
                redact_notifier_config(&mut config);
            }
            NotifierSummary {
                id: r.get(0),
                name: r.get(1),
                kind: r.get(2),
                config,
            }
        })
        .collect();
    Ok(Json(out))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<NotifierReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    let id = db::notifiers::upsert(&state.pool, &req.name, &req.kind, &req.config)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateResp { id }))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
    Path(id): Path<i64>,
) -> Result<Json<NotifierSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, name, kind, config_json FROM notifiers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    let mut config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();
    if auth == AuthSource::Token {
        redact_notifier_config(&mut config);
    }
    Ok(Json(NotifierSummary {
        id: row.get(0),
        name: row.get(1),
        kind: row.get(2),
        config,
    }))
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(mut req): Json<NotifierReq>,
) -> Result<StatusCode, StatusCode> {
    let row = sqlx::query("SELECT config_json FROM notifiers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let current_config_str: String = row.get(0);
    let current_config: serde_json::Value =
        serde_json::from_str(&current_config_str).unwrap_or_default();
    unredact_notifier_config(&mut req.config, &current_config);

    db::notifiers::upsert(&state.pool, &req.name, &req.kind, &req.config)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    sqlx::query("DELETE FROM notifiers WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let row = sqlx::query("SELECT kind, config_json FROM notifiers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let kind: String = row.get(0);
    let config_str: String = row.get(1);
    let config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();

    let notifier =
        notifiers::build(&kind, &config).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    notifier
        .send("transcoderr test notification", &serde_json::Value::Null)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
