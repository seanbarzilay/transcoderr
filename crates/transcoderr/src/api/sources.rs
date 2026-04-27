use crate::api::auth::AuthSource;
use crate::arr;
use crate::db;
use crate::http::AppState;
use axum::Extension;
use axum::{extract::{Path, State}, http::StatusCode, Json};
use rand::RngCore;
use sqlx::Row;
use transcoderr_api_types::{CreatedIdResp as CreateResp, CreateSourceReq, SourceSummary, UpdateSourceReq};
use tracing;

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
            .map_err(|e| {
                tracing::error!(kind = %req.kind, error = ?e, "failed to construct *arr client");
                StatusCode::BAD_GATEWAY
            })?;
        let notification = client
            .create_notification(arr_kind, &req.name, &webhook_url, &secret_token)
            .await
            .map_err(|e| {
                tracing::error!(kind = %req.kind, name = %req.name, error = ?e,
                    "failed to create *arr notification");
                StatusCode::BAD_GATEWAY
            })?;

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
    let row = match db::sources::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let old_cfg: serde_json::Value =
        serde_json::from_str(&row.config_json).unwrap_or_default();
    let mut new_cfg = match req.config {
        Some(ref c) => c.clone(),
        None => old_cfg.clone(),
    };

    if !new_cfg.is_object() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let new_name = req.name.clone().unwrap_or_else(|| row.name.clone());
    let arr_kind = arr::Kind::parse(&row.kind);

    let needs_reprovision = arr_kind.is_some()
        && old_cfg.get("arr_notification_id").is_some()
        && (old_cfg.get("base_url") != new_cfg.get("base_url")
            || old_cfg.get("api_key") != new_cfg.get("api_key")
            || new_name != row.name);

    if needs_reprovision {
        let arr_kind = arr_kind.unwrap();
        let old_id = old_cfg
            .get("arr_notification_id")
            .and_then(|v| v.as_i64())
            .unwrap();

        if let (Some(old_base), Some(old_key)) = (
            old_cfg.get("base_url").and_then(|v| v.as_str()),
            old_cfg.get("api_key").and_then(|v| v.as_str()),
        ) {
            if let Ok(c) = arr::Client::new(old_base, old_key) {
                if let Err(e) = c.delete_notification(old_id).await {
                    tracing::warn!(source_id = id, old_id, error = %e,
                        "failed to delete old *arr webhook during update; proceeding");
                }
            }
        }

        let new_base = new_cfg
            .get("base_url")
            .and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let new_key = new_cfg
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let webhook_url = format!("{}/webhook/{}", state.public_url, row.kind);
        let client = arr::Client::new(new_base, new_key).map_err(|e| {
            tracing::error!(source_id = id, error = ?e,
                "failed to construct *arr client during update");
            StatusCode::BAD_GATEWAY
        })?;
        let new_n = client
            .create_notification(arr_kind, &new_name, &webhook_url, &row.secret_token)
            .await
            .map_err(|e| {
                tracing::error!(source_id = id, error = ?e,
                    "failed to provision new *arr webhook on update");
                StatusCode::BAD_GATEWAY
            })?;

        if let Some(obj) = new_cfg.as_object_mut() {
            obj.insert("arr_notification_id".into(), serde_json::json!(new_n.id));
        }
    }

    // Auto-provisioned rows (arr_kind matched AND we stamped an
    // arr_notification_id) own their secret_token — it's the *arr->transcoderr
    // shared password, regenerated only on create. Manual rows (radarr/sonarr/
    // lidarr kind without an arr_notification_id, or non-arr kinds) accept
    // explicit secret_token updates; the "***" sentinel is preserved.
    let auto_provisioned =
        arr_kind.is_some() && old_cfg.get("arr_notification_id").is_some();
    let new_secret = match (auto_provisioned, req.secret_token.as_deref()) {
        (true, _) => row.secret_token.clone(),
        (false, Some(s)) if s != "***" => s.to_string(),
        _ => row.secret_token.clone(),
    };
    let cfg_str = serde_json::to_string(&new_cfg)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    sqlx::query("UPDATE sources SET name = ?, config_json = ?, secret_token = ? WHERE id = ?")
        .bind(&new_name)
        .bind(&cfg_str)
        .bind(&new_secret)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let row = match db::sources::get_by_id(&state.pool, id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let cfg: serde_json::Value =
        serde_json::from_str(&row.config_json).unwrap_or_default();
    let arr_kind = arr::Kind::parse(&row.kind);
    let notification_id = cfg.get("arr_notification_id").and_then(|v| v.as_i64());

    if let (Some(_arr_kind), Some(notification_id)) = (arr_kind, notification_id) {
        let base_url = cfg.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
        let api_key = cfg.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
        if !base_url.is_empty() && !api_key.is_empty() {
            match arr::Client::new(base_url, api_key) {
                Ok(client) => match client.delete_notification(notification_id).await {
                    Ok(()) => tracing::info!(source_id = id, notification_id, "deleted *arr webhook"),
                    Err(e) => tracing::warn!(source_id = id, notification_id, error = %e,
                        "failed to delete *arr webhook; proceeding with local delete"),
                },
                Err(e) => tracing::warn!(source_id = id, error = %e,
                    "failed to construct *arr client; proceeding with local delete"),
            }
        }
    }

    sqlx::query("DELETE FROM sources WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await
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
