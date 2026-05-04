use crate::{db, http::AppState};
use axum::{extract::State, http::StatusCode, Json};
use serde_json::Value;
use sqlx::Row;
use std::collections::HashMap;

pub async fn get_all(
    State(state): State<AppState>,
) -> Result<Json<HashMap<String, String>>, StatusCode> {
    let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut out = HashMap::new();
    for r in rows {
        let key: String = r.get(0);
        let val: String = r.get(1);
        out.insert(key, val);
    }
    Ok(Json(out))
}

pub async fn patch(
    State(state): State<AppState>,
    Json(body): Json<HashMap<String, Value>>,
) -> Result<StatusCode, StatusCode> {
    // Special handling: if auth.enabled is being set to "true", require auth.password
    if let Some(en_val) = body.get("auth.enabled") {
        let en_str = match en_val {
            Value::String(s) => s.as_str() == "true",
            Value::Bool(b) => *b,
            _ => false,
        };
        if en_str {
            // Must also provide auth.password
            let password = match body.get("auth.password") {
                Some(Value::String(p)) if !p.is_empty() => p.clone(),
                _ => return Err(StatusCode::BAD_REQUEST),
            };
            let hash = crate::api::auth::hash_password(&password)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            db::settings::set(&state.pool, "auth.password_hash", &hash)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    for (key, val) in &body {
        // Skip auth.password - never store it raw
        if key == "auth.password" {
            continue;
        }
        let val_str = match val {
            Value::String(s) => s.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };
        db::settings::set(&state.pool, key, &val_str)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    Ok(StatusCode::NO_CONTENT)
}
