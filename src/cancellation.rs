//! Cooperative cancellation registry for in-flight jobs.
//!
//! When a job starts running, the worker calls [`JobCancellations::register`] to mint
//! a fresh [`CancellationToken`] for that job_id. The engine threads the token through
//! to step impls (via `Context::cancel`); ffmpeg-running steps race the child process's
//! exit against `token.cancelled()` and SIGKILL the child if cancellation fires.
//!
//! When the user clicks Cancel in the UI, the API handler calls [`cancel`] which
//! triggers the token, propagating cancellation through the engine and ffmpeg.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct JobCancellations {
    inner: Arc<Mutex<HashMap<i64, CancellationToken>>>,
}

impl JobCancellations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh cancellation token for `job_id`. Replaces any existing entry.
    pub fn register(&self, job_id: i64) -> CancellationToken {
        let token = CancellationToken::new();
        self.inner.lock().unwrap().insert(job_id, token.clone());
        token
    }

    /// Trigger cancellation for `job_id`. Returns `true` if a token was registered.
    pub fn cancel(&self, job_id: i64) -> bool {
        match self.inner.lock().unwrap().get(&job_id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Remove the token for `job_id`. Called by the worker after a run completes.
    pub fn unregister(&self, job_id: i64) {
        self.inner.lock().unwrap().remove(&job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_cancel_propagates() {
        let reg = JobCancellations::new();
        let t = reg.register(42);
        assert!(!t.is_cancelled());
        assert!(reg.cancel(42));
        assert!(t.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_unknown_job_returns_false() {
        let reg = JobCancellations::new();
        assert!(!reg.cancel(999));
    }

    #[tokio::test]
    async fn unregister_drops_entry() {
        let reg = JobCancellations::new();
        let _t = reg.register(1);
        reg.unregister(1);
        assert!(!reg.cancel(1));
    }
}
