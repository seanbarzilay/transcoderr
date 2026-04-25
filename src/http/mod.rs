use crate::config::Config;
use axum::{extract::State, routing::post, Extension, Router};
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
    pub hw_caps: Arc<tokio::sync::RwLock<crate::hw::HwCaps>>,
    pub hw_devices: crate::hw::semaphores::DeviceRegistry,
    pub bus: crate::bus::Bus,
}

pub fn router(state: AppState) -> Router {
    let dedup = Arc::new(dedup::DedupCache::new(Duration::from_secs(300)));
    Router::new()
        .route("/webhook/radarr", post(webhook_radarr::handle))
        .route("/webhook/sonarr", post(webhook_sonarr::handle))
        .route("/webhook/lidarr", post(webhook_lidarr::handle))
        .route("/webhook/:name", post(webhook_generic::handle))
        .route("/api/hw", axum::routing::get(get_hw))
        .route("/api/hw/reprobe", axum::routing::post(reprobe_hw))
        .nest("/api", crate::api::router(state.clone()))
        .layer(Extension(dedup))
        .with_state(state)
        .fallback(crate::static_assets::serve)
}

async fn get_hw(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let g = state.hw_caps.read().await.clone();
    axum::Json(g)
}

async fn reprobe_hw(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let new_caps = crate::hw::probe::probe().await;
    let _ = crate::db::snapshot_hw_caps(&state.pool, &new_caps).await;
    *state.hw_caps.write().await = new_caps.clone();
    axum::Json(new_caps)
}
