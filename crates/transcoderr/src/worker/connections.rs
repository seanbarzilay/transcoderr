//! Coordinator-side registry of active worker WebSocket connections.
//! Two indexes:
//!
//! - `senders: worker_id -> mpsc::Sender<Envelope>`: how the
//!   `dispatch::remote::RemoteRunner` pushes a `step_dispatch` to a
//!   specific worker.
//!
//! - `inbox: correlation_id -> mpsc::Sender<InboundStepEvent>`: how
//!   the WS receive loop demuxes inbound `step_progress` /
//!   `step_complete` frames back to the `RemoteRunner` that's
//!   awaiting them.
//!
//! Both maps are guarded by a small `Connections` API. Cleanup uses
//! a `ConnectionGuard` RAII helper so the registry stays consistent
//! even if a WS task panics.

use crate::worker::protocol::{Envelope, StepComplete, StepProgressMsg};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

#[derive(Debug, Clone)]
pub enum InboundStepEvent {
    Progress(StepProgressMsg),
    Complete(StepComplete),
}

#[derive(Default)]
pub struct Connections {
    senders: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    inbox: Arc<RwLock<HashMap<String, mpsc::Sender<InboundStepEvent>>>>,
}

impl Connections {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    /// Register a worker's outbound channel. Returns a guard whose
    /// drop removes the entry — call `register_sender` from the WS
    /// handler and bind the guard to the task's stack so a panic
    /// still cleans up.
    pub async fn register_sender(
        self: &Arc<Self>,
        worker_id: i64,
        tx: mpsc::Sender<Envelope>,
    ) -> SenderGuard {
        self.senders.write().await.insert(worker_id, tx);
        SenderGuard {
            map: self.senders.clone(),
            worker_id,
        }
    }

    /// Send an envelope to the worker. Returns Err if the worker
    /// isn't registered (e.g. just disconnected) or its channel is
    /// closed.
    pub async fn send_to_worker(
        &self,
        worker_id: i64,
        env: Envelope,
    ) -> Result<(), &'static str> {
        let map = self.senders.read().await;
        let tx = map.get(&worker_id).ok_or("worker not connected")?;
        tx.send(env).await.map_err(|_| "worker channel closed")?;
        Ok(())
    }

    /// True if a sender for this worker is currently registered.
    pub async fn is_connected(&self, worker_id: i64) -> bool {
        self.senders.read().await.contains_key(&worker_id)
    }

    /// Register an inbox for a single dispatch. Returns the Receiver
    /// and a guard that removes the inbox on drop.
    pub async fn register_inbox(
        self: &Arc<Self>,
        correlation_id: String,
    ) -> (mpsc::Receiver<InboundStepEvent>, InboxGuard) {
        let (tx, rx) = mpsc::channel(8);
        self.inbox
            .write()
            .await
            .insert(correlation_id.clone(), tx);
        let guard = InboxGuard {
            map: self.inbox.clone(),
            correlation_id,
        };
        (rx, guard)
    }

    /// Forward an inbound step_progress / step_complete frame to the
    /// awaiting RemoteRunner. Drops silently if no inbox is
    /// registered (the runner already gave up / cleaned up).
    pub async fn forward_inbound(
        &self,
        correlation_id: &str,
        event: InboundStepEvent,
    ) {
        let map = self.inbox.read().await;
        if let Some(tx) = map.get(correlation_id) {
            let _ = tx.send(event).await;
        } else {
            tracing::debug!(correlation_id, "no inbox for inbound step frame; dropping");
        }
    }
}

pub struct SenderGuard {
    map: Arc<RwLock<HashMap<i64, mpsc::Sender<Envelope>>>>,
    worker_id: i64,
}

impl Drop for SenderGuard {
    fn drop(&mut self) {
        // Drop is sync; spawn a small task to remove from the async map.
        let map = self.map.clone();
        let worker_id = self.worker_id;
        tokio::spawn(async move {
            map.write().await.remove(&worker_id);
        });
    }
}

pub struct InboxGuard {
    map: Arc<RwLock<HashMap<String, mpsc::Sender<InboundStepEvent>>>>,
    correlation_id: String,
}

impl Drop for InboxGuard {
    fn drop(&mut self) {
        let map = self.map.clone();
        let id = self.correlation_id.clone();
        tokio::spawn(async move {
            map.write().await.remove(&id);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::protocol::{Heartbeat, Message};

    #[tokio::test]
    async fn register_and_send_to_worker() {
        let conns = Connections::new();
        let (tx, mut rx) = mpsc::channel(4);
        let _guard = conns.register_sender(42, tx).await;
        assert!(conns.is_connected(42).await);

        let env = Envelope {
            id: "x".into(),
            message: Message::Heartbeat(Heartbeat {}),
        };
        conns.send_to_worker(42, env.clone()).await.unwrap();
        assert_eq!(rx.recv().await.unwrap(), env);
    }

    #[tokio::test]
    async fn sender_guard_removes_on_drop() {
        let conns = Connections::new();
        let (tx, _rx) = mpsc::channel(4);
        {
            let _guard = conns.register_sender(7, tx).await;
            assert!(conns.is_connected(7).await);
        }
        // Drop spawns an async cleanup; give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!conns.is_connected(7).await);
    }

    #[tokio::test]
    async fn inbox_round_trip() {
        let conns = Connections::new();
        let (mut rx, _guard) = conns.register_inbox("c1".into()).await;
        let ev = InboundStepEvent::Progress(StepProgressMsg {
            job_id: 1,
            step_id: "s".into(),
            kind: "progress".into(),
            payload: serde_json::json!({"pct": 10}),
        });
        conns.forward_inbound("c1", ev.clone()).await;
        let received = rx.recv().await.unwrap();
        match (received, ev) {
            (InboundStepEvent::Progress(a), InboundStepEvent::Progress(b)) => {
                assert_eq!(a, b);
            }
            _ => panic!("variant mismatch"),
        }
    }

    #[tokio::test]
    async fn forward_inbound_to_missing_inbox_is_silent() {
        let conns = Connections::new();
        // Should not panic.
        conns
            .forward_inbound(
                "nope",
                InboundStepEvent::Complete(StepComplete {
                    job_id: 1,
                    step_id: "s".into(),
                    status: "ok".into(),
                    error: None,
                    ctx_snapshot: Some("{}".into()),
                }),
            )
            .await;
    }
}
