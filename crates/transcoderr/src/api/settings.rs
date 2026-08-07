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
    // The `auth.` namespace is never written by the generic loop below.
    // It holds `auth.password_hash` (the credential `login` verifies
    // against) and `auth.enabled` (the flag `require_auth` reads), so a
    // caller able to set them verbatim could install a password hash of
    // their own choosing or switch authentication off entirely. Both
    // transitions are handled explicitly here, server-side, from values
    // this handler derives rather than values the body supplies.
    if let Some(en_val) = body.get("auth.enabled") {
        let want_enabled = match en_val {
            Value::String(s) => s.as_str() == "true",
            Value::Bool(b) => *b,
            _ => false,
        };
        if want_enabled {
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
            db::settings::set(&state.pool, "auth.enabled", "true")
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        } else {
            db::settings::set(&state.pool, "auth.enabled", "false")
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
    }

    for (key, val) in &body {
        // `auth.password` must never be stored raw, and the rest of the
        // `auth.` namespace is set above from server-derived values only.
        if key.starts_with("auth.") {
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
