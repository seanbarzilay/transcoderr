pub mod auth;
pub mod dryrun;
pub mod flows;
pub mod jobs;
pub mod notifiers;
pub mod plugins;
pub mod runs;
pub mod settings;
pub mod sources;

use crate::http::AppState;
use axum::{
    extract::State,
    middleware::from_fn_with_state,
    routing::{delete, get, patch, post},
    Router,
};
use tower_cookies::CookieManagerLayer;

pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/auth/login",  post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me",     get(auth::me));

    let protected = Router::new()
        .route("/auth/tokens",        get(auth::list_tokens).post(auth::create_token))
        .route("/auth/tokens/:id",    delete(auth::delete_token))
        .route("/flows",              get(flows::list).post(flows::create))
        .route("/flows/:id",          get(flows::get).put(flows::update).delete(flows::delete))
        .route("/flows/parse",        post(flows::parse))
        .route("/hw",                 get(hw_get))
        .route("/hw/reprobe",         post(hw_reprobe))
        .route("/version",            get(version_get))
        .route("/runs",               get(runs::list))
        .route("/runs/:id",           get(runs::get))
        .route("/runs/:id/events",    get(runs::events))
        .route("/runs/:id/cancel",    post(runs::cancel))
        .route("/runs/:id/rerun",     post(runs::rerun))
        .route("/jobs/:id",           get(jobs::get))
        .route("/sources",            get(sources::list).post(sources::create))
        .route("/sources/:id",        get(sources::get).put(sources::update).delete(sources::delete))
        .route("/sources/:id/test-fire", post(sources::test_fire))
        .route("/plugins",            get(plugins::list))
        .route("/plugins/:id",        patch(plugins::update))
        .route("/notifiers",          get(notifiers::list).post(notifiers::create))
        .route("/notifiers/:id",      get(notifiers::get).put(notifiers::update).delete(notifiers::delete))
        .route("/notifiers/:id/test", post(notifiers::test))
        .route("/settings",           get(settings::get_all).patch(settings::patch))
        .route("/dry-run",            post(dryrun::dry_run))
        .route("/stream",             axum::routing::get(crate::bus::sse::stream))
        .route_layer(from_fn_with_state(state.clone(), auth::require_auth));

    public.merge(protected).layer(CookieManagerLayer::new())
}

async fn hw_get(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let g = state.hw_caps.read().await.clone();
    axum::Json(g)
}

async fn hw_reprobe(State(state): State<AppState>) -> axum::Json<crate::hw::HwCaps> {
    let new_caps = crate::hw::probe::probe().await;
    let _ = crate::db::snapshot_hw_caps(&state.pool, &new_caps).await;
    *state.hw_caps.write().await = new_caps.clone();
    axum::Json(new_caps)
}

async fn version_get() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
}
