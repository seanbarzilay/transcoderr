//! `/api/worker/plugins/:name/tarball` — coordinator serves the
//! cached source tarball to a connected worker. Auth is
//! Bearer-on-Request against `db::workers::secret_token` (same shape
//! as `/api/worker/connect`'s upgrade path).

use crate::db;
use crate::http::AppState;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
};
use tokio_util::io::ReaderStream;

/// GET /api/worker/plugins/:name/tarball
///
/// Auth: `Authorization: Bearer <worker.secret_token>`. The worker's
/// `coordinator_token` from worker.toml goes here.
///
/// Responses:
/// - 200 + `application/x-gzip` body — the cached tarball
/// - 401 — missing/invalid Bearer
/// - 404 — plugin not found in `db::plugins WHERE enabled=1`, or the
///         cache file is missing on disk
/// - 500 — DB error
pub async fn tarball(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    // 1. Bearer auth against workers.secret_token.
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();
    let _row = db::workers::get_by_token(&state.pool, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // 2. Lookup plugin row.
    let plugins = db::plugins::list_enabled(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let plugin = plugins
        .into_iter()
        .find(|p| p.name == name)
        .ok_or(StatusCode::NOT_FOUND)?;
    let sha = plugin.tarball_sha256.ok_or(StatusCode::NOT_FOUND)?;

    // 3. Open cache file.
    let cache_path = state
        .cfg
        .data_dir
        .join("plugins")
        .join(".tarball-cache")
        .join(format!("{name}-{sha}.tar.gz"));
    let file = match tokio::fs::File::open(&cache_path).await {
        Ok(f) => f,
        Err(_) => {
            tracing::warn!(?cache_path, "tarball cache miss");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // 4. Stream the file.
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-gzip")
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
