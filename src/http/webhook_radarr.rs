use crate::db;
use crate::http::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct RadarrPayload {
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "movieFile", default)]
    pub movie_file: Option<RadarrMovieFile>,
    #[serde(default)]
    pub movie: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovieFile {
    pub path: String,
}

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    raw: Json<Value>,
) -> Result<StatusCode, StatusCode> {
    // Auth.
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let expected = format!("Bearer {}", state.cfg.radarr.bearer_token);
    if auth != expected {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let payload: RadarrPayload = serde_json::from_value(raw.0.clone())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let event = normalize_event(&payload.event_type);
    let Some(file) = payload.movie_file else { return Ok(StatusCode::ACCEPTED); };

    let flows = db::flows::list_enabled_for_radarr(&state.pool, &event)
        .await.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let raw_str = serde_json::to_string(&raw.0).unwrap_or_default();
    for flow in flows {
        let _ = db::jobs::insert(&state.pool, flow.id, flow.version, "radarr",
            &file.path, &raw_str).await;
    }
    Ok(StatusCode::ACCEPTED)
}

fn normalize_event(e: &str) -> String {
    // Radarr uses "Download", "MovieFileDelete", "Test", etc. We lowercase and map a couple.
    match e {
        "Download" | "MovieFileImported" => "downloaded".to_string(),
        "MovieFileDelete" => "deleted".to_string(),
        other => other.to_lowercase(),
    }
}
