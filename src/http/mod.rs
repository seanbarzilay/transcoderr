use crate::config::Config;
use axum::{routing::post, Router};
use sqlx::SqlitePool;
use std::sync::Arc;

pub mod webhook_radarr;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/webhook/radarr", post(webhook_radarr::handle))
        .with_state(state)
}
