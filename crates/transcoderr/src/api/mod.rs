pub mod arr_browse;
pub mod auth;
pub mod dryrun;
pub mod flows;
pub mod jobs;
pub mod notifiers;
pub mod plugin_catalogs;
pub mod plugins;
pub mod runs;
pub mod settings;
pub mod sources;
pub mod workers;

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
        .route("/auth/me",     get(auth::me))
        .route("/worker/connect", get(workers::connect));

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
        .route("/sources/:id/movies", get(arr_browse::movies))
        .route("/sources/:id/series", get(arr_browse::series))
        .route("/sources/:id/series/:series_id", get(arr_browse::series_get))
        .route("/sources/:id/series/:series_id/episodes", get(arr_browse::episodes))
        .route("/sources/:id/refresh", post(arr_browse::refresh))
        .route("/sources/:id/transcode", post(arr_browse::transcode))
        .route("/plugins",            get(plugins::list))
        .route("/plugins/:id",        get(plugins::get).delete(plugins::uninstall))
        .route("/plugin-catalog-entries",   get(plugins::browse))
        .route("/plugin-catalog-entries/:catalog_id/:name/install", post(plugins::install))
        .route("/plugin-catalogs",          get(plugin_catalogs::list).post(plugin_catalogs::create))
        .route("/plugin-catalogs/:id",      delete(plugin_catalogs::delete))
        .route("/plugin-catalogs/:id/refresh", post(plugin_catalogs::refresh))
        .route("/notifiers",          get(notifiers::list).post(notifiers::create))
        .route("/notifiers/:id",      get(notifiers::get).put(notifiers::update).delete(notifiers::delete))
        .route("/notifiers/:id/test", post(notifiers::test))
        .route("/settings",           get(settings::get_all).patch(settings::patch))
        .route("/dry-run",            post(dryrun::dry_run))
        .route("/workers",            get(workers::list).post(workers::create))
        .route("/workers/:id",        patch(workers::patch).delete(workers::delete))
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
