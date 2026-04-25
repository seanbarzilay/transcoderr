use crate::config::Config;
use axum::{routing::post, Extension, Router};
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};

pub mod dedup;
pub mod webhook_generic;
pub mod webhook_lidarr;
pub mod webhook_radarr;
pub mod webhook_sonarr;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    let dedup = Arc::new(dedup::DedupCache::new(Duration::from_secs(300)));
    Router::new()
        .route("/webhook/radarr", post(webhook_radarr::handle))
        .route("/webhook/sonarr", post(webhook_sonarr::handle))
        .route("/webhook/lidarr", post(webhook_lidarr::handle))
        .route("/webhook/:name", post(webhook_generic::handle))
        .layer(Extension(dedup))
        .with_state(state)
}
