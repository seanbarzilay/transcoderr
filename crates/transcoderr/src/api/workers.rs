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

#[derive(serde::Deserialize)]
pub struct PatchReq {
    pub enabled: Option<bool>,
}

/// PATCH /api/workers/:id — currently the only mutable field is
/// `enabled`. Returns the updated row as `WorkerSummary` (un-redacted —
/// PATCH is session/UI-authed, not a token-replay surface). 404 if id
/// missing; 400 if no settable fields supplied.
pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<PatchReq>,
) -> Result<Json<WorkerSummary>, StatusCode> {
    let Some(enabled) = req.enabled else {
        return Err(StatusCode::BAD_REQUEST);
    };
    let n = db::workers::set_enabled(&state.pool, id, enabled)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, id, "failed to set workers.enabled");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if n == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    let row = db::workers::get_by_id(&state.pool, id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // PATCH is UI-driven (session-authed) — return un-redacted, same
    // policy as create() returning the cleartext mint.
    Ok(Json(row_to_summary(row, false)))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetPathMappingsReq {
    pub rules: Vec<crate::path_mapping::PathMapping>,
}

#[derive(Debug, Serialize)]
pub struct SetPathMappingsResp {
    pub id: i64,
    /// Echo of the canonical (trailing-slash-normalised) rules that
    /// were stored. Empty array if the operator cleared mappings.
    pub rules: Vec<crate::path_mapping::PathMapping>,
}

/// PUT /api/workers/:id/path-mappings — set or clear the per-worker
/// path-mapping rules. Empty `rules` array clears (column → NULL).
/// Refuses `kind='local'` rows with 400. Same auth as the rest of
/// `/api/workers` (lives in the protected Router branch).
pub async fn set_path_mappings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<SetPathMappingsReq>,
) -> Result<Json<SetPathMappingsResp>, StatusCode> {
    // Reject any rule with empty from/to.
    for rule in &req.rules {
        if rule.from.trim().is_empty() || rule.to.trim().is_empty() {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    // Normalise (trailing slashes) by round-tripping through PathMappings.
    let mappings = crate::path_mapping::PathMappings::from_rules(req.rules);
    let canonical = mappings.rules().to_vec();

    // Empty rules → store NULL; non-empty → re-serialise the canonical
    // (trailing-slash-stripped) form.
    let json: Option<String> = if canonical.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(&canonical)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
        )
    };

    let n = db::workers::update_path_mappings(&state.pool, id, json.as_deref())
        .await
        .map_err(|e| {
            tracing::error!(id, error = ?e, "failed to update path_mappings_json");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if n == 0 {
        // Either the id is missing OR the row is kind='local'. Either way,
        // 400 is the right answer for the operator-facing error: the
        // request was rejected because the target worker can't accept
        // mappings.
        return Err(StatusCode::BAD_REQUEST);
    }

    // Refresh the Connections cache so a subsequent dispatch picks up
    // the new mappings without re-reading the DB.
    state
        .connections
        .set_path_mappings(id, mappings)
        .await;

    Ok(Json(SetPathMappingsResp { id, rules: canonical }))
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

async fn handle_connection(state: AppState, socket: WebSocket, worker_id: i64) {
    use futures::{SinkExt, StreamExt};
    let (mut ws_sink, mut ws_stream) = socket.split();

    // Outbound mpsc: anyone (including the dispatch::remote runner)
    // can push an Envelope here and the sender task forwards it to
    // the wire.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Envelope>(32);

    // 1. Wait for the register frame inline (read via the stream half).
    // Two failure modes are common and benign: the timeout fires
    // (slow / broken worker), or the stream closes cleanly before
    // sending Register (`worker::connection::probe_token` does this on
    // every successful boot to validate the cached token without
    // triggering a full registration). Both log at debug to avoid
    // operator-facing log spam on healthy boots.
    let register = match tokio::time::timeout(REGISTER_TIMEOUT, recv_message(&mut ws_stream)).await {
        Ok(Ok(Envelope { id, message: Message::Register(r) })) => (id, r),
        _ => {
            tracing::debug!(worker_id, "no register frame within {REGISTER_TIMEOUT:?} (likely a probe); closing");
            let _ = ws_sink.close().await;
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
        let _ = ws_sink.close().await;
        return;
    }

    // Capture the worker's advertised step kinds so the dispatcher
    // can filter eligible workers per step kind. Mirror image of the
    // Message::Register receive-loop arm below (Piece 5).
    state
        .connections
        .record_available_steps(worker_id, register_payload.available_steps.clone())
        .await;

    // 3. Register the outbound channel in `Connections` (RAII cleanup
    //    on drop). We register BEFORE sending register_ack so the
    //    worker's first frames-after-ack already see a live entry.
    let _sender_guard = state.connections.register_sender(worker_id, out_tx.clone()).await;

    // 4. Spawn the sender task: drains `out_rx` -> ws_sink.
    let sender_task = tokio::spawn(async move {
        while let Some(env) = out_rx.recv().await {
            match serde_json::to_string(&env) {
                Ok(s) => {
                    if ws_sink.send(WsMessage::Text(s)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = ?e, "failed to serialise outbound envelope");
                    break;
                }
            }
        }
        let _ = ws_sink.close().await;
    });

    // 5. Send the register_ack with the same correlation id, via the
    //    new outbound path.
    // Build the worker's intended plugin manifest from db::plugins.
    let manifest: Vec<crate::worker::protocol::PluginInstall> =
        match db::plugins::list_enabled(&state.pool).await {
            Ok(plugins) => plugins
                .into_iter()
                .filter_map(|p| {
                    let sha = p.tarball_sha256?;
                    Some(crate::worker::protocol::PluginInstall {
                        tarball_url: format!(
                            "{}/api/worker/plugins/{}/tarball",
                            state.public_url, p.name
                        ),
                        name: p.name,
                        version: p.version,
                        sha256: sha,
                    })
                })
                .collect(),
            Err(e) => {
                tracing::warn!(error = ?e, "list_enabled failed; sending empty manifest");
                Vec::new()
            }
        };

    let ack = Envelope {
        id: correlation_id,
        message: Message::RegisterAck(RegisterAck {
            worker_id,
            plugin_install: manifest,
        }),
    };
    if out_tx.send(ack).await.is_err() {
        tracing::warn!(worker_id, "sender task closed before register_ack");
        sender_task.abort();
        return;
    }

    tracing::info!(worker_id, name = %register_payload.name, "worker registered");

    // 6. Inbound receive loop. Heartbeat / step_progress /
    //    step_complete are the variants we handle; everything else
    //    logs a warn.
    while let Ok(env) = recv_message(&mut ws_stream).await {
        let correlation_id = env.id.clone();
        match env.message {
            Message::Heartbeat(_) => {
                if let Err(e) = db::workers::record_heartbeat(&state.pool, worker_id).await {
                    tracing::warn!(worker_id, error = ?e, "failed to record heartbeat");
                }
            }
            Message::Register(r) => {
                // Re-register from worker — typically fired after a
                // plugin_sync::sync rebuilds its registry. Update the
                // worker row + the in-memory available_steps map. NO
                // register_ack response (would oscillate — see spec
                // distributed-piece-5).
                let hw_caps_json = serde_json::to_string(&r.hw_caps)
                    .unwrap_or_else(|_| "null".into());
                let plugin_manifest_json = serde_json::to_string(&r.plugin_manifest)
                    .unwrap_or_else(|_| "[]".into());
                if let Err(e) = db::workers::record_register(
                    &state.pool,
                    worker_id,
                    &hw_caps_json,
                    &plugin_manifest_json,
                )
                .await
                {
                    tracing::warn!(worker_id, error = ?e, "re-register: record_register failed");
                }
                state
                    .connections
                    .record_available_steps(worker_id, r.available_steps)
                    .await;
                tracing::debug!(worker_id, "re-register processed");
            }
            Message::StepProgress(p) => {
                state
                    .connections
                    .forward_inbound(
                        &correlation_id,
                        crate::worker::connections::InboundStepEvent::Progress(p),
                    )
                    .await;
            }
            Message::StepComplete(c) => {
                state
                    .connections
                    .forward_inbound(
                        &correlation_id,
                        crate::worker::connections::InboundStepEvent::Complete(c),
                    )
                    .await;
            }
            other => {
                tracing::warn!(worker_id, ?other, "unexpected message; ignoring");
            }
        }
    }
    tracing::info!(worker_id, "worker disconnected");
    sender_task.abort();
    // _sender_guard drops here -> Connections::senders entry removed.
}

async fn recv_message<S>(stream: &mut S) -> anyhow::Result<Envelope>
where
    S: futures::Stream<Item = Result<WsMessage, axum::Error>> + Unpin,
{
    use futures::StreamExt;
    while let Some(msg) = stream.next().await {
        match msg? {
            WsMessage::Text(t) => return Ok(serde_json::from_str(&t)?),
            WsMessage::Close(_) => anyhow::bail!("connection closed"),
            _ => continue,
        }
    }
    anyhow::bail!("stream ended");
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
