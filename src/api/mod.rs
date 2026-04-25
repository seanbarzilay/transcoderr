pub mod auth;
pub mod flows;

use crate::http::AppState;
use axum::{middleware::from_fn_with_state, routing::{get, post}, Router};
use tower_cookies::CookieManagerLayer;

pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/auth/login",  post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me",     get(auth::me));

    let protected = Router::new()
        .route("/flows",       get(flows::list).post(flows::create))
        .route("/flows/:id",   get(flows::get).put(flows::update).delete(flows::delete))
        .route("/flows/parse", post(flows::parse))
        .route_layer(from_fn_with_state(state.clone(), auth::require_auth));

    public.merge(protected).layer(CookieManagerLayer::new())
}
