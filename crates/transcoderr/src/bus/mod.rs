use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub mod sse;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "topic", content = "data")]
pub enum Event {
    JobState { id: i64, status: String, label: Option<String> },
    RunEvent { job_id: i64, step_id: Option<String>, worker_id: Option<i64>, kind: String, payload: serde_json::Value },
    Queue    { pending: i64, running: i64 },
}

#[derive(Clone)]
pub struct Bus {
    pub tx: broadcast::Sender<Event>,
}

impl Default for Bus {
    fn default() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }
}

impl Bus {
    pub fn send(&self, ev: Event) {
        let _ = self.tx.send(ev);
    }
}
