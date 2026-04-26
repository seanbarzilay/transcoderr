use crate::{db, http::AppState, http::auth_extract, http::dedup::DedupCache};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct Payload {
    #[serde(rename = "eventType")]
    event_type: String,
    #[serde(rename = "episodeFile")]
    episode_file: Option<EpisodeFile>,
}

#[derive(Debug, Deserialize)]
struct EpisodeFile {
    path: String,
}

pub async fn handle(
    State(state): State<AppState>,
    Extension(dedup): Extension<Arc<DedupCache>>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    let token = auth_extract::extract_token(&headers);
    let source = db::sources::get_by_kind_and_token(&state.pool, "sonarr", &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let payload: Payload =
        serde_json::from_value(raw.0.clone()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let event = match payload.event_type.as_str() {
        "Download" | "EpisodeFileImport" => "downloaded",
        _ => return Ok(StatusCode::ACCEPTED),
    };
    let Some(file) = payload.episode_file else {
        return Ok(StatusCode::ACCEPTED);
    };
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    if !dedup.observe(source.id, &file.path, &raw_str) {
        return Ok(StatusCode::ACCEPTED);
    }
    let flows = db::flows::list_enabled_for_sonarr(&state.pool, event)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    for flow in flows {
        let _ = db::jobs::insert_with_source(
            &state.pool,
            flow.id,
            flow.version,
            source.id,
            "sonarr",
            &file.path,
            &raw_str,
        )
        .await;
    }
    Ok(StatusCode::ACCEPTED)
}
