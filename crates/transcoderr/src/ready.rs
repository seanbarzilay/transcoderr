use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone, Default)]
pub struct Readiness {
    inner: Arc<RwLock<bool>>,
}

impl Readiness {
    pub fn new() -> Self { Self::default() }
    pub async fn mark_ready(&self) { *self.inner.write().await = true; }
    pub async fn is_ready(&self) -> bool { *self.inner.read().await }
}
