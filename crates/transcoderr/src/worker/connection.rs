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

/// Run the worker connection loop. Never returns. On every disconnect
/// (clean or error), waits for the current backoff and retries. On a
/// clean close (`Ok(())` from `connect_once`), the backoff resets.
pub async fn run<F>(url: String, token: String, build_register: F) -> !
where
    F: Fn() -> Envelope + Send + Sync,
{
    let mut backoff = BACKOFF_INITIAL;

    loop {
        match connect_once(&url, &token, &build_register).await {
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

async fn connect_once<F>(
    url: &str,
    token: &str,
    build_register: &F,
) -> anyhow::Result<()>
where
    F: Fn() -> Envelope,
{
    let mut req = url.into_client_request()?;
    req.headers_mut().insert(
        AUTHORIZATION,
        format!("Bearer {token}").parse()?,
    );

    let (ws, _resp) = tokio_tungstenite::connect_async(req).await?;
    tracing::info!(url, "worker WS connected");
    let (mut ws_sink, mut ws_stream) = ws.split();

    // Outbound mpsc → sender task → WS sink. Heartbeats and step
    // results both go through this channel so the receive loop
    // never blocks on the WS sink.
    let (outbound_tx, mut outbound_rx) =
        tokio::sync::mpsc::channel::<Envelope>(32);
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

    let register = build_register();
    if outbound_tx.send(register).await.is_err() {
        sender_task.abort();
        anyhow::bail!("failed to enqueue register frame");
    }

    let ack_raw = ws_stream.next().await
        .ok_or_else(|| anyhow::anyhow!("stream closed before register_ack"))??;
    let ack: Envelope = match ack_raw {
        WsMessage::Text(s) => serde_json::from_str(&s)?,
        WsMessage::Close(_) => {
            sender_task.abort();
            anyhow::bail!("server closed before register_ack");
        }
        other => {
            sender_task.abort();
            anyhow::bail!("unexpected non-text frame from server: {other:?}");
        }
    };
    match ack.message {
        Message::RegisterAck(_) => {
            tracing::info!("worker register acknowledged");
        }
        other => {
            sender_task.abort();
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
                    return Ok(());
                }
            }
            frame = ws_stream.next() => {
                match frame {
                    Some(Ok(WsMessage::Close(_))) => {
                        sender_task.abort();
                        return Ok(());
                    }
                    Some(Ok(WsMessage::Text(s))) => {
                        match serde_json::from_str::<Envelope>(&s) {
                            Ok(env) => {
                                let correlation = env.id.clone();
                                match env.message {
                                    Message::StepDispatch(dispatch) => {
                                        let tx_for_step = outbound_tx.clone();
                                        tokio::spawn(async move {
                                            crate::worker::executor::handle_step_dispatch(
                                                tx_for_step,
                                                correlation,
                                                dispatch,
                                            )
                                            .await;
                                        });
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
                        return Err(e.into());
                    }
                    None => {
                        sender_task.abort();
                        return Ok(());
                    }
                }
            }
        }
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
