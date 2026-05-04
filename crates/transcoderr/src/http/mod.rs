use crate::config::Config;
use axum::{extract::State, routing::post, Extension, Router};
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};

pub mod auth_extract;
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
    pub ffmpeg_caps: Arc<crate::ffmpeg_caps::FfmpegCaps>,
    pub bus: crate::bus::Bus,
    pub ready: crate::ready::Readiness,
    pub metrics: std::sync::Arc<crate::metrics::Metrics>,
    pub cancellations: crate::cancellation::JobCancellations,
    pub public_url: std::sync::Arc<String>,
    pub arr_cache: std::sync::Arc<crate::arr::cache::ArrCache>,
    pub catalog_client: std::sync::Arc<crate::plugins::catalog::CatalogClient>,
    pub runtime_checker: std::sync::Arc<crate::plugins::runtime::RuntimeChecker>,
    pub connections: std::sync::Arc<crate::worker::connections::Connections>,
}

pub fn router(state: AppState, dedup_window: Duration) -> Router {
    let dedup = Arc::new(dedup::DedupCache::new(dedup_window));
    Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route("/readyz", axum::routing::get(readyz_handler))
        .route("/metrics", axum::routing::get(metrics_handler))
        .route("/webhook/radarr", post(webhook_radarr::handle))
        .route("/webhook/sonarr", post(webhook_sonarr::handle))
        .route("/webhook/lidarr", post(webhook_lidarr::handle))
        .route("/webhook/:name", post(webhook_generic::handle))
        .nest("/api", crate::api::router(state.clone()))
        .layer(Extension(dedup))
        .with_state(state)
        .fallback(crate::static_assets::serve)
}

async fn readyz_handler(State(state): State<AppState>) -> axum::http::StatusCode {
    if state.ready.is_ready().await {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn metrics_handler(State(state): State<AppState>) -> String {
    state.metrics.render()
}
