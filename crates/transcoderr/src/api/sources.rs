use crate::api::auth::AuthSource;
use crate::arr;
use crate::db;
use crate::http::AppState;
use axum::Extension;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use rand::RngCore;
use sqlx::Row;
use transcoderr_api_types::{CreatedIdResp as CreateResp, CreateSourceReq, SourceSummary, UpdateSourceReq};

/// Returns a copy of `config` with `api_key` masked to `"***"` when
/// `redact` is true. Mirrors the secret-redaction policy applied to
/// `secret_token` for token-authed (MCP) callers.
fn redact_config(config: &serde_json::Value, redact: bool) -> serde_json::Value {
    if !redact {
        return config.clone();
    }
    let mut out = config.clone();
    if let Some(obj) = out.as_object_mut() {
        if obj.contains_key("api_key") {
            obj.insert("api_key".into(), serde_json::json!("***"));
        }
    }
    out
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<SourceSummary>>, StatusCode> {
    let rows = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources ORDER BY name")
        .fetch_all(&state.pool).await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let redact = auth == AuthSource::Token;
    let out = rows.into_iter().map(|r| {
        let config_str: String = r.get(3);
        let secret: String = r.get(4);
        let cfg: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();
        SourceSummary {
            id: r.get(0),
            kind: r.get(1),
            name: r.get(2),
            config: redact_config(&cfg, redact),
            secret_token: if redact { "***".into() } else { secret },
        }
    }).collect();
    Ok(Json(out))
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateSourceReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    // Auto-provision branch: kind is one of radarr/sonarr/lidarr.
    if let Some(arr_kind) = arr::Kind::parse(&req.kind) {
        let obj = req.config.as_object().ok_or(StatusCode::BAD_REQUEST)?;
        let base_url = obj
            .get("base_url")
            .and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let api_key = obj
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;

        // 32-byte random hex secret used as the *arr -> transcoderr webhook
        // shared password and as the row's `secret_token`.
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let secret_token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

        let webhook_url = format!("{}/webhook/{}", state.public_url, req.kind);
        let client = arr::Client::new(base_url, api_key)
            .map_err(|_| StatusCode::BAD_GATEWAY)?;
        let notification = client
            .create_notification(arr_kind, &req.name, &webhook_url, &secret_token)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        let mut cfg = req.config.clone();
        if let Some(map) = cfg.as_object_mut() {
            map.insert("arr_notification_id".into(), serde_json::json!(notification.id));
        }

        let id = db::sources::insert(&state.pool, &req.kind, &req.name, &cfg, &secret_token)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(CreateResp { id }));
    }

    // Manual path: generic / webhook / etc.
    let id = db::sources::insert(&state.pool, &req.kind, &req.name, &req.config, &req.secret_token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(CreateResp { id }))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
    Path(id): Path<i64>,
) -> Result<Json<SourceSummary>, StatusCode> {
    let row = sqlx::query("SELECT id, kind, name, config_json, secret_token FROM sources WHERE id = ?")
        .bind(id).fetch_optional(&state.pool).await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let config_str: String = row.get(3);
    let secret: String = row.get(4);
    let redact = auth == AuthSource::Token;
    let cfg: serde_json::Value = serde_json::from_str(&config_str).unwrap_or_default();
    Ok(Json(SourceSummary {
        id: row.get(0),
        kind: row.get(1),
        name: row.get(2),
        config: redact_config(&cfg, redact),
        secret_token: if redact { "***".into() } else { secret },
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
    let secret_token: String = match req.secret_token {
        Some(s) if s == "***" => row.get(2),  // ignore redaction sentinel from token-authed callers
        Some(s) => s,
        None => row.get(2),
    };

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
