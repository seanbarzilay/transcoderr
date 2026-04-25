pub mod auth;

use crate::http::AppState;
use axum::{routing::{get, post}, Router};
use tower_cookies::CookieManagerLayer;

pub fn router(_state: AppState) -> Router<AppState> {
    // Public auth routes. Protected routes (requiring auth middleware) will be
    // added in subsequent tasks once there are routes to protect.
    Router::new()
        .route("/auth/login",  post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me",     get(auth::me))
        .layer(CookieManagerLayer::new())
}
