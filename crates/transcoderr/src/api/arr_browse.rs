//! Browse-and-transcode proxy endpoints. Each GET handler validates
//! the source is auto-provisioned (radarr/sonarr/lidarr kind with
//! `arr_notification_id` + `base_url` + `api_key` in config), reads
//! through the in-memory TTL cache, and returns a trimmed view of the
//! *arr's library.

use crate::arr;
use crate::db;
use crate::http::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use transcoderr_api_types::{ApiError, MoviesPage};

/// Validation result: returns the row plus the parsed kind, base_url, api_key.
pub(super) async fn browseable_source(
    state: &AppState,
    id: i64,
) -> Result<(db::sources::SourceRow, arr::Kind, String, String), (StatusCode, Json<ApiError>)> {
    let row = db::sources::get_by_id(&state.pool, id)
        .await
        .map_err(|e| {
            tracing::error!(source_id = id, error = ?e, "db error in browseable_source");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("db.error", "database error")),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ApiError::new("source.not_found", "source not found")),
            )
        })?;
    let kind = arr::Kind::parse(&row.kind).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.not_browseable",
                "source kind does not support browsing",
            )),
        )
    })?;
    let cfg: serde_json::Value =
        serde_json::from_str(&row.config_json).unwrap_or(serde_json::Value::Null);
    if cfg.get("arr_notification_id").and_then(|v| v.as_i64()).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.not_browseable",
                "source is not auto-provisioned",
            )),
        ));
    }
    let base_url = cfg
        .get("base_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new(
                    "source.not_browseable",
                    "config.base_url missing",
                )),
            )
        })?
        .to_string();
    let api_key = cfg
        .get("api_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ApiError::new(
                    "source.not_browseable",
                    "config.api_key missing",
                )),
            )
        })?
        .to_string();
    Ok((row, kind, base_url, api_key))
}

/// Map an `arr::Client` HTTP error to a 502 with the *arr's body in
/// the error chain (logged at `error` level for operator visibility).
pub(super) fn arr_call_error(source_id: i64, e: anyhow::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!(source_id, error = ?e, "*arr proxy call failed");
    (
        StatusCode::BAD_GATEWAY,
        Json(ApiError::new("arr.upstream", &format!("{e}"))),
    )
}

#[derive(Debug, Deserialize)]
pub struct BrowseParams {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: Option<String>, // "title" | "year"; default "title"
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

pub(super) const CACHE_KEY_MOVIES: &str = "movies";

pub async fn movies(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
    Query(params): Query<BrowseParams>,
) -> Result<Json<MoviesPage>, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    if kind != arr::Kind::Radarr {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.wrong_kind",
                "this endpoint is only for radarr sources",
            )),
        ));
    }

    // Cache holds the trimmed Vec<MovieSummary> as a Value for type-erasure.
    // NOTE: ArrCache methods are sync (see Task 2) — no `.await` here.
    let cached = state.arr_cache.get(source_id, CACHE_KEY_MOVIES);
    let trimmed: Vec<transcoderr_api_types::MovieSummary> = if let Some(v) = cached {
        serde_json::from_value(v).unwrap_or_default()
    } else {
        let client = arr::Client::new(&base_url, &api_key)
            .map_err(|e| arr_call_error(source_id, e))?;
        let movies = client
            .list_movies()
            .await
            .map_err(|e| arr_call_error(source_id, e))?;
        let trimmed: Vec<_> = movies.into_iter().map(|m| m.into_summary(&base_url)).collect();
        let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
        state.arr_cache.put(source_id, CACHE_KEY_MOVIES, v);
        trimmed
    };

    let page = filter_sort_paginate_movies(trimmed, &params);
    Ok(Json(page))
}

fn filter_sort_paginate_movies(
    mut items: Vec<transcoderr_api_types::MovieSummary>,
    params: &BrowseParams,
) -> MoviesPage {
    if let Some(q) = params.search.as_ref().filter(|s| !s.is_empty()) {
        let needle = q.to_lowercase();
        items.retain(|m| m.title.to_lowercase().contains(&needle));
    }
    match params.sort.as_deref().unwrap_or("title") {
        "year" => items.sort_by(|a, b| b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0))),
        _ => items.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
    let total = items.len() as i64;
    let limit = params.limit.unwrap_or(48).clamp(1, 200);
    let page = params.page.unwrap_or(1).max(1);
    let start = ((page - 1) * limit) as usize;
    let end = (start + limit as usize).min(items.len());
    let window = if start < items.len() {
        items[start..end].to_vec()
    } else {
        vec![]
    };
    MoviesPage { items: window, total, page, limit }
}
