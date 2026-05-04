use crate::{db, http::auth_extract, http::dedup::DedupCache, http::AppState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct RadarrPayload {
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "movieFile", default)]
    pub movie_file: Option<RadarrMovieFile>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovieFile {
    pub path: String,
}

pub async fn handle(
    State(state): State<AppState>,
    Extension(dedup): Extension<Arc<DedupCache>>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let token = auth_extract::extract_token(&headers);
    let source = db::sources::get_by_kind_and_token(&state.pool, "radarr", &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let payload: RadarrPayload =
        serde_json::from_value(raw.0.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let event = match payload.event_type.as_str() {
        "Download" | "MovieFileImported" => "downloaded",
        _ => return Ok(StatusCode::ACCEPTED),
    };
    let Some(file) = payload.movie_file else {
        return Ok(StatusCode::ACCEPTED);
    };
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    if !dedup.observe(source.id, &file.path, &raw_str) {
        return Ok(StatusCode::ACCEPTED);
    }
    let flows = db::flows::list_enabled_for_radarr(&state.pool, event)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for flow in flows {
        let _ = db::jobs::insert_with_source(
            &state.pool,
            flow.id,
            flow.version,
            source.id,
            "radarr",
            &file.path,
            &raw_str,
        )
        .await;
    }
    Ok(StatusCode::ACCEPTED)
}
