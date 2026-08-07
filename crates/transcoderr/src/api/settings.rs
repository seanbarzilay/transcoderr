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
        // Never leave the server. `auth.password_hash` is the Argon2 PHC
        // string `login` verifies against; handing it to any caller that
        // can read settings turns an API credential into an offline
        // cracking target for the operator's actual password. The web UI
        // filtered this key client-side, which only ever hid it from the
        // page — it was still on the wire.
        //
        // `auth.enabled` is deliberately still returned: it is not a
        // secret (GET /api/auth/me exposes the same fact unauthenticated
        // as `auth_required`), and the Settings page needs it to render
        // the auth row and the password field.
        if key == "auth.password_hash" {
            continue;
        }
        let val: String = r.get(1);
        out.insert(key, val);
    }
    Ok(Json(out))
}

/// Errors carry a body so the Settings page can show the operator why a
/// save failed — `web/src/api/client.ts` builds its message from the
/// response text, and a bare StatusCode renders as `400 Bad Request: `.
type PatchError = (StatusCode, &'static str);

const E_INTERNAL: PatchError = (
    StatusCode::INTERNAL_SERVER_ERROR,
    "could not write settings",
);

pub async fn patch(
    State(state): State<AppState>,
    Json(body): Json<HashMap<String, Value>>,
) -> Result<StatusCode, PatchError> {
    // The `auth.` namespace is never written by the generic loop below.
    // It holds `auth.password_hash` (the credential `login` verifies
    // against) and `auth.enabled` (the flag `require_auth` reads), so a
    // caller able to set them verbatim could install a password hash of
    // their own choosing or switch authentication off entirely. Both
    // transitions are handled explicitly here, server-side, from values
    // this handler derives rather than values the body supplies.

    // A supplied password is stored whatever else the body asks for, so a
    // scripted `PATCH {"auth.password": "..."}` rotates the credential
    // instead of returning 204 having done nothing.
    let supplied_password = match body.get("auth.password") {
        Some(Value::String(p)) if !p.is_empty() => Some(p.as_str()),
        _ => None,
    };
    if let Some(p) = supplied_password {
        let hash = crate::api::auth::hash_password(p).map_err(|_| E_INTERNAL)?;
        db::settings::set(&state.pool, "auth.password_hash", &hash)
            .await
            .map_err(|_| E_INTERNAL)?;
    }

    if let Some(en_val) = body.get("auth.enabled") {
        let want_enabled = match en_val {
            Value::String(s) => s.as_str() == "true",
            Value::Bool(b) => *b,
            _ => false,
        };
        if want_enabled {
            // Turning auth on needs a credential to check against. A save
            // that only touches unrelated settings while auth is already
            // configured carries no password and must still succeed; a
            // fresh install with no stored hash must not, or the operator
            // locks themselves out of a server that now demands a password
            // nobody set. The migration seeds an empty hash, hence the
            // is_empty check rather than a presence check.
            if supplied_password.is_none() {
                let existing = db::settings::get(&state.pool, "auth.password_hash")
                    .await
                    .map_err(|_| E_INTERNAL)?
                    .unwrap_or_default();
                if existing.is_empty() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "set a password to enable authentication",
                    ));
                }
            }
            db::settings::set(&state.pool, "auth.enabled", "true")
                .await
                .map_err(|_| E_INTERNAL)?;
        } else {
            db::settings::set(&state.pool, "auth.enabled", "false")
                .await
                .map_err(|_| E_INTERNAL)?;
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
            .map_err(|_| E_INTERNAL)?;
    }
    Ok(StatusCode::NO_CONTENT)
}
