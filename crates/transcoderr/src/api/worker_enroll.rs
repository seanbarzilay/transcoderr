//! `POST /api/worker/enroll` — unauthenticated enrollment endpoint
//! used by workers that found the coordinator via mDNS.
//!
//! Trust model: open enrollment on the LAN. Documented in
//! `docs/superpowers/specs/2026-05-03-worker-auto-discovery-design.md`
//! (decision Q2-A). The endpoint is idempotent only in the sense that
//! repeated calls each insert a fresh row with a fresh token —
//! collisions on `name` are accepted (existing UI concern, not new).

use crate::db;
use crate::http::AppState;
use axum::{extract::State, http::StatusCode, Json};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct EnrollReq {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct EnrollResp {
    pub id: i64,
    /// Cleartext token. Returned exactly once at enrollment.
    pub secret_token: String,
    /// Pre-built WebSocket URL the worker should dial. Built from
    /// the coordinator's resolved `public_url` with the scheme flipped
    /// to `ws://` / `wss://`.
    pub ws_url: String,
}

pub async fn enroll(
    State(state): State<AppState>,
    Json(req): Json<EnrollReq>,
) -> Result<Json<EnrollResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let id = db::workers::insert_remote(&state.pool, &req.name, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "enroll: failed to insert worker row");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let ws_url = http_to_ws(&state.public_url) + "/api/worker/connect";

    tracing::info!(id, name = %req.name, "worker enrolled via auto-discovery");

    Ok(Json(EnrollResp { id, secret_token: token, ws_url }))
}

/// Flip `http://` → `ws://`, `https://` → `wss://`. Anything else passes
/// through (caller will see the malformed URL when it tries to dial).
fn http_to_ws(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_to_ws_handles_both_schemes() {
        assert_eq!(http_to_ws("http://192.168.1.50:8765"), "ws://192.168.1.50:8765");
        assert_eq!(http_to_ws("https://example.com"), "wss://example.com");
        // Trailing slash policy: caller appends "/api/worker/connect", so
        // we leave the trailing slash (or absence) alone.
        assert_eq!(http_to_ws("http://x/"), "ws://x/");
    }
}
