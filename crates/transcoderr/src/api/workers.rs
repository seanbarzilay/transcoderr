//! REST endpoints for the workers registry. The WebSocket upgrade
//! handler (`/api/worker/connect`) lives in this same file (added in
//! Task 5) — one file per resource matches the existing
//! `api/sources.rs` / `api/notifiers.rs` pattern.

use crate::api::auth::AuthSource;
use crate::db;
use crate::http::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use rand::RngCore;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct WorkerSummary {
    pub id: i64,
    pub name: String,
    pub kind: String,
    /// Redacted to "***" for token-authed callers; clear text for
    /// session callers (the UI). Local-worker rows always serialize as
    /// `null` since they have no token.
    pub secret_token: Option<String>,
    pub hw_caps: Option<serde_json::Value>,
    pub plugin_manifest: Option<serde_json::Value>,
    pub enabled: bool,
    pub last_seen_at: Option<i64>,
    pub created_at: i64,
}

fn row_to_summary(row: db::workers::WorkerRow, redact: bool) -> WorkerSummary {
    WorkerSummary {
        id: row.id,
        name: row.name,
        kind: row.kind,
        secret_token: row.secret_token.map(|t| if redact { "***".to_string() } else { t }),
        hw_caps: row
            .hw_caps_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        plugin_manifest: row
            .plugin_manifest_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        enabled: row.enabled != 0,
        last_seen_at: row.last_seen_at,
        created_at: row.created_at,
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthSource>,
) -> Result<Json<Vec<WorkerSummary>>, StatusCode> {
    let rows = db::workers::list_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let redact = auth == AuthSource::Token;
    Ok(Json(rows.into_iter().map(|r| row_to_summary(r, redact)).collect()))
}

#[derive(serde::Deserialize)]
pub struct CreateReq {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateResp {
    pub id: i64,
    /// One-time-display: this is the only response that ever contains
    /// the cleartext token. Subsequent reads return `***`.
    pub secret_token: String,
}

/// Mint a new remote worker. Returns `{id, secret_token}` once;
/// subsequent reads via `/api/workers` redact the token for token-authed
/// callers. The token is a 32-byte hex string (matches the format used
/// by the auto-provisioned *arr secret_tokens at `api/sources.rs`).
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateReq>,
) -> Result<Json<CreateResp>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let id = db::workers::insert_remote(&state.pool, &req.name, &token)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "failed to insert worker row");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(CreateResp { id, secret_token: token }))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, StatusCode> {
    let removed = db::workers::delete_remote(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if removed == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- WebSocket upgrade -----------------------------------------------------

use crate::worker::protocol::{Envelope, Message, RegisterAck};
use axum::{
    extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    http::HeaderMap,
    response::Response,
};
use std::time::Duration;

/// Window we wait for the worker to send its `register` after the WS
/// upgrade completes. Anything beyond this is treated as a misbehaving
/// client and the connection closes.
const REGISTER_TIMEOUT: Duration = Duration::from_secs(5);

/// Workers older than this without an inbound frame are marked stale by
/// the idle sweep task. Stays loose to absorb a missed heartbeat or two
/// over a flaky link.
pub const STALE_AFTER_SECS: i64 = 90;

/// GET /api/worker/connect — upgrade to WebSocket. Auth is Bearer-on-the
/// upgrade-request (workers don't have session cookies). Token must
/// match a row in the `workers` table.
pub async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();

    let row = db::workers::get_by_token(&state.pool, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    Ok(ws.on_upgrade(move |socket| handle_connection(state, socket, row.id)))
}

async fn handle_connection(state: AppState, mut socket: WebSocket, worker_id: i64) {
    // 1. Wait up to REGISTER_TIMEOUT for the `register` frame.
    let register = match tokio::time::timeout(REGISTER_TIMEOUT, recv_message(&mut socket)).await {
        Ok(Ok(Envelope { id, message: Message::Register(r) })) => (id, r),
        _ => {
            tracing::warn!(worker_id, "no valid register within {REGISTER_TIMEOUT:?}; closing");
            let _ = socket.close().await;
            return;
        }
    };
    let (correlation_id, register_payload) = register;

    // 2. Persist the registration.
    let hw_caps_json = serde_json::to_string(&register_payload.hw_caps).unwrap_or_else(|_| "null".into());
    let plugin_manifest_json =
        serde_json::to_string(&register_payload.plugin_manifest).unwrap_or_else(|_| "[]".into());
    if let Err(e) = db::workers::record_register(
        &state.pool,
        worker_id,
        &hw_caps_json,
        &plugin_manifest_json,
    )
    .await
    {
        tracing::error!(worker_id, error = ?e, "failed to record register");
        let _ = socket.close().await;
        return;
    }

    // 3. Send the register_ack with the same correlation id.
    let ack = Envelope {
        id: correlation_id,
        message: Message::RegisterAck(RegisterAck {
            worker_id,
            plugin_install: vec![], // Piece 4 fills this in
        }),
    };
    if !send_message(&mut socket, &ack).await {
        return;
    }

    tracing::info!(worker_id, name = %register_payload.name, "worker registered");

    // 4. Receive loop. Piece 1 only handles heartbeats; Pieces 3+ add
    //    step_progress / step_complete.
    while let Ok(env) = recv_message(&mut socket).await {
        match env.message {
            Message::Heartbeat(_) => {
                if let Err(e) = db::workers::record_heartbeat(&state.pool, worker_id).await {
                    tracing::warn!(worker_id, error = ?e, "failed to record heartbeat");
                }
            }
            other => {
                tracing::warn!(worker_id, ?other, "unexpected message; ignoring");
            }
        }
    }
    tracing::info!(worker_id, "worker disconnected");
}

async fn recv_message(socket: &mut WebSocket) -> anyhow::Result<Envelope> {
    while let Some(msg) = socket.recv().await {
        match msg? {
            WsMessage::Text(t) => return Ok(serde_json::from_str(&t)?),
            WsMessage::Close(_) => anyhow::bail!("connection closed"),
            _ => continue,
        }
    }
    anyhow::bail!("stream ended");
}

async fn send_message(socket: &mut WebSocket, env: &Envelope) -> bool {
    match serde_json::to_string(env) {
        Ok(s) => socket.send(WsMessage::Text(s)).await.is_ok(),
        Err(e) => {
            tracing::error!(error = ?e, "failed to serialise outbound envelope");
            false
        }
    }
}

/// Background task: every 60s, log when any remote worker has gone
/// stale (last_seen older than STALE_AFTER_SECS). Piece 1 just logs;
/// Piece 6 reassigns in-flight jobs.
pub async fn spawn_idle_sweep(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            if let Ok(rows) = db::workers::list_all(&state.pool).await {
                let now = chrono::Utc::now().timestamp();
                for row in rows {
                    if row.kind != "remote" {
                        continue;
                    }
                    if let Some(seen) = row.last_seen_at {
                        if now - seen > STALE_AFTER_SECS {
                            tracing::debug!(
                                worker_id = row.id, name = %row.name,
                                age_secs = now - seen, "worker stale"
                            );
                        }
                    }
                }
            }
        }
    });
}
