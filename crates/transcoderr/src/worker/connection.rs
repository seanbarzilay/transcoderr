//! WebSocket dial + reconnect loop. The daemon (in `daemon.rs`) calls
//! `run` once; this function never returns until the process is
//! killed — it loops forever, opening a fresh connection on every
//! disconnect with exponential backoff.

use crate::worker::protocol::{Envelope, Heartbeat, Message};
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Context the worker connection needs for plugin sync AND for
/// building Register envelopes (initial + post-sync). Threaded from
/// `daemon::run` → `connection::run` → `connect_once`.
#[derive(Clone)]
pub struct ConnectionContext {
    pub plugins_dir: std::path::PathBuf,
    pub coordinator_token: String,
    /// Worker's display name (from `worker.toml` or hostname). Used
    /// in every Register envelope.
    pub name: String,
    /// Hardware capabilities, frozen at boot. Re-register reuses the
    /// same value (hardware doesn't change mid-process).
    pub hw_caps: serde_json::Value,
}

/// Build a fresh `Register` envelope from the live registry +
/// on-disk plugin manifest. Called twice per connection lifecycle:
/// once at the pre-handshake send, and once after each
/// `plugin_sync::sync` completes. Both call sites need an
/// up-to-the-moment snapshot of `available_steps`.
pub async fn build_register_envelope(ctx: &ConnectionContext) -> Envelope {
    use crate::worker::protocol::{PluginManifestEntry, Register};

    let plugin_manifest: Vec<PluginManifestEntry> = match crate::plugins::discover(&ctx.plugins_dir)
    {
        Ok(found) => found
            .into_iter()
            .map(|d| PluginManifestEntry {
                name: d.manifest.name.clone(),
                version: d.manifest.version.clone(),
                sha256: None,
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = ?e, "register: plugin discovery failed; reporting empty manifest");
            Vec::new()
        }
    };
    let available_steps = crate::steps::registry::list_step_names().await;

    Envelope {
        id: format!("reg-{}", uuid::Uuid::new_v4()),
        message: Message::Register(Register {
            name: ctx.name.clone(),
            version: env!("CARGO_PKG_VERSION").into(),
            hw_caps: ctx.hw_caps.clone(),
            available_steps,
            plugin_manifest,
        }),
    }
}

/// Run the worker connection loop. Never returns. On every disconnect
/// (clean or error), waits for the current backoff and retries. On a
/// clean close (`Ok(())` from `connect_once`), the backoff resets.
pub async fn run(url: String, token: String, ctx: ConnectionContext) -> ! {
    let mut backoff = BACKOFF_INITIAL;

    loop {
        match connect_once(&url, &token, &ctx).await {
            Ok(()) => {
                tracing::info!("worker connection closed cleanly; reconnecting");
                backoff = BACKOFF_INITIAL;
            }
            Err(e) => {
                tracing::warn!(error = %e, "worker connection error");
            }
        }

        tracing::info!(?backoff, "waiting before reconnect");
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

async fn connect_once(url: &str, token: &str, ctx: &ConnectionContext) -> anyhow::Result<()> {
    let mut req = url.into_client_request()?;
    req.headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {token}").parse()?);

    let (ws, _resp) = tokio_tungstenite::connect_async(req).await?;
    tracing::info!(url, "worker WS connected");
    let (mut ws_sink, mut ws_stream) = ws.split();

    // Outbound mpsc → sender task → WS sink. Heartbeats and step
    // results both go through this channel so the receive loop
    // never blocks on the WS sink.
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel::<Envelope>(32);
    let sender_task = tokio::spawn(async move {
        while let Some(env) = outbound_rx.recv().await {
            match serde_json::to_string(&env) {
                Ok(s) => {
                    if ws_sink.send(WsMessage::Text(s)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "worker outbound serialise failed");
                }
            }
        }
    });

    // Plugin-sync queue: single-slot. Latest manifest wins.
    let sync_slot: std::sync::Arc<
        tokio::sync::Mutex<Option<Vec<crate::worker::protocol::PluginInstall>>>,
    > = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let sync_notify = std::sync::Arc::new(tokio::sync::Notify::new());

    // Per-connection cancel registry. Keyed by correlation_id (the
    // step_dispatch envelope.id). `handle_step_dispatch` registers a
    // fresh token at dispatch start; the receive loop's StepCancel
    // arm fires it. Lives for the connection's lifetime.
    let step_cancellations: std::sync::Arc<
        tokio::sync::RwLock<std::collections::HashMap<String, tokio_util::sync::CancellationToken>>,
    > = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    // Sync worker: drain the slot whenever notified, run plugin_sync::sync,
    // then re-register so the coordinator's `connections.available_steps`
    // sees the new step kinds. Lives for the connection's lifetime;
    // aborted on disconnect.
    let sync_task = {
        let ctx_for_sync = ctx.clone();
        let outbound_for_sync = outbound_tx.clone();
        let slot = sync_slot.clone();
        let notify = sync_notify.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                let manifest = {
                    let mut g = slot.lock().await;
                    g.take()
                };
                if let Some(m) = manifest {
                    crate::worker::plugin_sync::sync(
                        &ctx_for_sync.plugins_dir,
                        m,
                        &ctx_for_sync.coordinator_token,
                    )
                    .await;
                    // Re-register so the coordinator sees fresh
                    // available_steps. Fire-and-forget — coordinator
                    // does NOT respond with another register_ack
                    // (would oscillate; see Piece 5 spec).
                    let env = build_register_envelope(&ctx_for_sync).await;
                    if let Err(e) = outbound_for_sync.send(env).await {
                        tracing::warn!(error = ?e, "post-sync re-register: outbound send failed");
                    }
                }
            }
        })
    };

    let register = build_register_envelope(ctx).await;
    if outbound_tx.send(register).await.is_err() {
        sender_task.abort();
        sync_task.abort();
        anyhow::bail!("failed to enqueue register frame");
    }

    let ack_raw = match ws_stream.next().await {
        Some(Ok(frame)) => frame,
        Some(Err(e)) => {
            sender_task.abort();
            sync_task.abort();
            return Err(e.into());
        }
        None => {
            sender_task.abort();
            sync_task.abort();
            anyhow::bail!("stream closed before register_ack");
        }
    };
    let ack: Envelope = match ack_raw {
        WsMessage::Text(s) => match serde_json::from_str(&s) {
            Ok(env) => env,
            Err(e) => {
                sender_task.abort();
                sync_task.abort();
                return Err(e.into());
            }
        },
        WsMessage::Close(_) => {
            sender_task.abort();
            sync_task.abort();
            anyhow::bail!("server closed before register_ack");
        }
        other => {
            sender_task.abort();
            sync_task.abort();
            anyhow::bail!("unexpected non-text frame from server: {other:?}");
        }
    };
    match ack.message {
        Message::RegisterAck(ack_payload) => {
            tracing::info!("worker register acknowledged");
            // Trigger initial sync unconditionally — even with an
            // empty manifest, full-mirror semantics require we
            // uninstall any stale local plugins (a worker reconnecting
            // after the operator deleted everything would otherwise
            // keep the stale set forever).
            let mut g = sync_slot.lock().await;
            *g = Some(ack_payload.plugin_install);
            drop(g);
            sync_notify.notify_one();
        }
        other => {
            sender_task.abort();
            sync_task.abort();
            anyhow::bail!("expected register_ack, got {other:?}");
        }
    }

    let mut ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let hb = Envelope {
                    id: format!("hb-{}", uuid::Uuid::new_v4()),
                    message: Message::Heartbeat(Heartbeat {}),
                };
                if outbound_tx.send(hb).await.is_err() {
                    sender_task.abort();
                    sync_task.abort();
                    return Ok(());
                }
            }
            frame = ws_stream.next() => {
                match frame {
                    Some(Ok(WsMessage::Close(_))) => {
                        sender_task.abort();
                        sync_task.abort();
                        return Ok(());
                    }
                    Some(Ok(WsMessage::Text(s))) => {
                        match serde_json::from_str::<Envelope>(&s) {
                            Ok(env) => {
                                let correlation = env.id.clone();
                                match env.message {
                                    Message::StepDispatch(dispatch) => {
                                        let tx_for_step = outbound_tx.clone();
                                        let cancellations = step_cancellations.clone();
                                        tokio::spawn(async move {
                                            crate::worker::executor::handle_step_dispatch(
                                                tx_for_step,
                                                correlation,
                                                dispatch,
                                                cancellations,
                                            )
                                            .await;
                                        });
                                    }
                                    Message::PluginSync(p) => {
                                        let mut g = sync_slot.lock().await;
                                        *g = Some(p.plugins);
                                        drop(g);
                                        sync_notify.notify_one();
                                    }
                                    Message::StepCancel(p) => {
                                        let map = step_cancellations.read().await;
                                        if let Some(token) = map.get(&correlation) {
                                            token.cancel();
                                            tracing::info!(
                                                job_id = p.job_id,
                                                step_id = %p.step_id,
                                                correlation_id = %correlation,
                                                "step cancel received"
                                            );
                                        } else {
                                            // Race: cancel arrived after step_complete already
                                            // fired (handle_step_dispatch removed the entry).
                                            // No-op; debug log only.
                                            tracing::debug!(
                                                correlation_id = %correlation,
                                                "step cancel for unknown correlation; dropped"
                                            );
                                        }
                                    }
                                    other => {
                                        tracing::warn!(?other, "worker received unexpected frame; ignoring");
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!(error = ?e, "worker failed to parse inbound frame");
                            }
                        }
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => {
                        sender_task.abort();
                        sync_task.abort();
                        return Err(e.into());
                    }
                    None => {
                        sender_task.abort();
                        sync_task.abort();
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Outcome of a single WS-upgrade probe used at boot to detect
/// cached-token rejection before entering the long-lived reconnect
/// loop. The probe dials, classifies the response, and closes
/// immediately — no Register frame is exchanged.
#[derive(Debug)]
pub enum ProbeOutcome {
    Ok,
    Unauthorized,
    Other(anyhow::Error),
}

/// Single WS-upgrade attempt against `url` with `token` as the Bearer.
/// Used by `daemon::run` to detect a stale cached token (HTTP 401)
/// before falling into the infinite reconnect loop. On `Other`, we
/// can still enter the reconnect loop because the failure is likely
/// transient (DNS, TCP, TLS, etc.).
pub async fn probe_token(url: &str, token: &str) -> ProbeOutcome {
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return ProbeOutcome::Other(anyhow::anyhow!("build request: {e}")),
    };
    let bearer = match format!("Bearer {token}").parse() {
        Ok(b) => b,
        Err(e) => return ProbeOutcome::Other(anyhow::anyhow!("build bearer header: {e}")),
    };
    req.headers_mut().insert(AUTHORIZATION, bearer);

    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => {
            let (mut sink, _) = ws.split();
            let _ = sink.send(WsMessage::Close(None)).await;
            ProbeOutcome::Ok
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(resp))
            if resp.status() == tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED =>
        {
            ProbeOutcome::Unauthorized
        }
        Err(e) => ProbeOutcome::Other(anyhow::anyhow!("probe: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps_at_max() {
        let mut b = BACKOFF_INITIAL;
        let mut history = vec![b];
        for _ in 0..10 {
            b = (b * 2).min(BACKOFF_MAX);
            history.push(b);
        }
        assert_eq!(
            history,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ]
        );
    }

    #[test]
    fn heartbeat_interval_is_30s() {
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(30));
    }
}
