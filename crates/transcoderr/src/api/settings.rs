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
        // Mirror the PATCH-side filter: every `auth.*` key is server-
        // internal credential / state. `auth.password_hash` is the
        // one that actually leaks crackable material if exposed (the
        // Argon2 hash an attacker could take offline), but blocking
        // the whole namespace also covers `auth.enabled` and any
        // future `auth.*` additions automatically. The Settings UI
        // gets `auth.enabled` from /api/auth/me's `auth_required`
        // boolean instead of reading it through this endpoint.
        if key.starts_with("auth.") {
            continue;
        }
        let val: String = r.get(1);
        out.insert(key, val);
    }
    Ok(Json(out))
}

pub async fn patch(
    State(state): State<AppState>,
    Json(body): Json<HashMap<String, Value>>,
) -> Result<StatusCode, StatusCode> {
    // Block ALL `auth.*` keys from arbitrary write through this endpoint.
    // These are server-internal credentials/state — `auth.password_hash`
    // (the Argon2 hash login verifies against), `auth.enabled` (the
    // global on/off switch), and any future auth.* additions. The one
    // legitimate operator-driven write — "enable auth and set the
    // initial password" — is handled by the special case below, which
    // takes the plaintext via `auth.password` and stores the hash
    // server-side. No request body should ever set `auth.password_hash`
    // directly.
    //
    // Without this filter, any authenticated caller (including a worker
    // token from POST /worker/enroll, which is on the unauthenticated
    // router) could PATCH `auth.password_hash` to a hash they generated
    // and then log in with the matching plaintext — full account
    // takeover. Reported by Enclave 2026-05-09 (critical).
    if let Some(en_val) = body.get("auth.enabled") {
        let en_bool = match en_val {
            Value::String(s) => s.as_str() == "true",
            Value::Bool(b) => *b,
            _ => false,
        };
        if en_bool {
            // Enabling auth requires the operator to also provide the
            // plaintext password they want to set. We hash it server-
            // side and store both `auth.password_hash` and
            // `auth.enabled = "true"` ourselves; the loop below skips
            // the entire `auth.*` namespace.
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
        }
        // Note: setting `auth.enabled = "false"` via this endpoint is
        // intentionally a silent no-op (the loop skips it). Disabling
        // auth from a request body would be the second leg of the
        // takeover chain — operators who legitimately need to disable
        // auth (e.g. forgot password) can edit the settings table in
        // the database directly.
    }

    for (key, val) in &body {
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
