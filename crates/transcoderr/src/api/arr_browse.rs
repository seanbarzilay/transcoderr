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
use transcoderr_api_types::{ApiError, MoviesPage, TranscodeReq, TranscodeResp, TranscodeRunRef};

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
    /// Exact codec match (case-insensitive). e.g. "x265", "h264".
    #[serde(default)]
    pub codec: Option<String>,
    /// Exact resolution match (case-insensitive). e.g. "3840x2160".
    #[serde(default)]
    pub resolution: Option<String>,
}

/// Distinct, lower-cased, sorted set of `(codec | resolution)` values
/// across the items' `file` field. Returned to the frontend so codec /
/// resolution dropdowns only offer values that actually exist in the
/// library.
fn distinct_codecs<I, F>(items: I, codec: F) -> Vec<String>
where
    I: IntoIterator,
    F: Fn(&I::Item) -> Option<String>,
{
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for it in items {
        if let Some(c) = codec(&it).filter(|s| !s.is_empty()) {
            set.insert(c.to_lowercase());
        }
    }
    set.into_iter().collect()
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
        // Browse pages exist to find files to transcode — drop entries
        // the *arr knows about but hasn't imported yet.
        let trimmed: Vec<_> = movies
            .into_iter()
            .map(|m| m.into_summary(&base_url))
            .filter(|m| m.has_file)
            .collect();
        let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
        state.arr_cache.put(source_id, CACHE_KEY_MOVIES, v);
        trimmed
    };

    // Compute available_* over the FULL cached library before
    // filtering, so the dropdowns stay stable when the user picks
    // a value (otherwise selecting "hevc" would shrink the codec
    // list to just ["hevc"] and you couldn't pick anything else).
    let available_codecs = distinct_codecs(trimmed.iter(), |m| {
        m.file.as_ref().and_then(|f| f.codec.clone())
    });
    let available_resolutions = distinct_codecs(trimmed.iter(), |m| {
        m.file.as_ref().and_then(|f| f.resolution.clone())
    });

    let page = filter_sort_paginate_movies(trimmed, &params, available_codecs, available_resolutions);
    Ok(Json(page))
}

fn filter_sort_paginate_movies(
    mut items: Vec<transcoderr_api_types::MovieSummary>,
    params: &BrowseParams,
    available_codecs: Vec<String>,
    available_resolutions: Vec<String>,
) -> MoviesPage {
    if let Some(q) = params.search.as_ref().filter(|s| !s.is_empty()) {
        let needle = q.to_lowercase();
        items.retain(|m| m.title.to_lowercase().contains(&needle));
    }
    if let Some(c) = params.codec.as_ref().filter(|s| !s.is_empty()) {
        let needle = c.to_lowercase();
        items.retain(|m| {
            m.file
                .as_ref()
                .and_then(|f| f.codec.as_ref())
                .map(|x| x.to_lowercase() == needle)
                .unwrap_or(false)
        });
    }
    if let Some(r) = params.resolution.as_ref().filter(|s| !s.is_empty()) {
        let needle = r.to_lowercase();
        items.retain(|m| {
            m.file
                .as_ref()
                .and_then(|f| f.resolution.as_ref())
                .map(|x| x.to_lowercase() == needle)
                .unwrap_or(false)
        });
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
    MoviesPage {
        items: window,
        total,
        page,
        limit,
        available_codecs,
        available_resolutions,
    }
}

const CACHE_KEY_SERIES: &str = "series";

/// Bounded parallelism for the per-series episode fan-out in `series()`.
/// Tuned so a ~100-series library takes ~3-5s cold against typical
/// homelab Sonarr latencies without overwhelming the *arr.
const SERIES_EPISODE_FETCH_CONCURRENCY: usize = 8;

pub async fn series(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
    Query(params): Query<BrowseParams>,
) -> Result<Json<transcoderr_api_types::SeriesPage>, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    if kind != arr::Kind::Sonarr {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.wrong_kind",
                "this endpoint is only for sonarr sources",
            )),
        ));
    }
    let cached = state.arr_cache.get(source_id, CACHE_KEY_SERIES);
    let trimmed: Vec<transcoderr_api_types::SeriesSummary> = if let Some(v) = cached {
        serde_json::from_value(v).unwrap_or_default()
    } else {
        let client = arr::Client::new(&base_url, &api_key)
            .map_err(|e| arr_call_error(source_id, e))?;
        let raw = client
            .list_series()
            .await
            .map_err(|e| arr_call_error(source_id, e))?;
        // Hide series that the *arr knows about but has never imported
        // an episode file for — nothing to transcode there.
        let mut trimmed: Vec<_> = raw
            .into_iter()
            .map(|s| s.into_summary(&base_url))
            .filter(|s| s.episode_file_count > 0)
            .collect();

        // Fan out per-series episode fetches in bounded parallelism so
        // the series list page can show codec/resolution badges and
        // filter on them. Costly cold (~3-5s for ~100 series) but warm
        // hits the cache. Episode lists from this fan-out are also
        // written into `episodes:{id}` to warm the series-detail page.
        use futures::stream::StreamExt;
        let ids: Vec<i64> = trimmed.iter().map(|s| s.id).collect();
        let results: Vec<(i64, anyhow::Result<Vec<transcoderr_api_types::EpisodeSummary>>)> =
            futures::stream::iter(ids)
                .map(|id| {
                    let c = client.clone();
                    async move {
                        let r = c.list_episodes(id).await.map(|raws| {
                            raws.into_iter()
                                .map(|e| e.into_summary())
                                .filter(|e| e.has_file)
                                .collect::<Vec<_>>()
                        });
                        (id, r)
                    }
                })
                .buffer_unordered(SERIES_EPISODE_FETCH_CONCURRENCY)
                .collect()
                .await;

        let mut by_id: std::collections::HashMap<i64, Vec<transcoderr_api_types::EpisodeSummary>> =
            std::collections::HashMap::new();
        for (id, r) in results {
            match r {
                Ok(eps) => {
                    by_id.insert(id, eps);
                }
                Err(e) => {
                    // Tolerate per-series episode fetch failures: that
                    // series just has no codec/resolution badges. The
                    // rest of the listing remains usable.
                    tracing::warn!(source_id, series_id = id, error = %e, "series codec aggregation: episode fetch failed");
                }
            }
        }

        for s in trimmed.iter_mut() {
            if let Some(eps) = by_id.get(&s.id) {
                s.codecs = distinct_codecs(eps.iter(), |e| {
                    e.file.as_ref().and_then(|f| f.codec.clone())
                });
                s.resolutions = distinct_codecs(eps.iter(), |e| {
                    e.file.as_ref().and_then(|f| f.resolution.clone())
                });
            }
        }

        // Warm the per-series episode cache so that clicking into a
        // series doesn't pay another round-trip.
        for (id, eps) in by_id {
            let key = format!("episodes:{id}");
            let v = serde_json::to_value(&eps).unwrap_or(serde_json::Value::Null);
            state.arr_cache.put(source_id, &key, v);
        }

        let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
        state.arr_cache.put(source_id, CACHE_KEY_SERIES, v);
        trimmed
    };

    // available_* over the full library, before filtering — keeps the
    // dropdowns stable when the user picks a value (same rationale as
    // the movies handler).
    let available_codecs = flatten_distinct(trimmed.iter().map(|s| &s.codecs));
    let available_resolutions = flatten_distinct(trimmed.iter().map(|s| &s.resolutions));

    Ok(Json(filter_sort_paginate_series(
        trimmed,
        &params,
        available_codecs,
        available_resolutions,
    )))
}

/// Flatten an iterator of `&Vec<String>` into a sorted, lower-cased,
/// distinct `Vec<String>`. Used to derive the SeriesPage available_*
/// dropdown sets from the per-series codec/resolution lists.
fn flatten_distinct<'a, I>(items: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a Vec<String>>,
{
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for v in items {
        for s in v {
            if !s.is_empty() {
                set.insert(s.to_lowercase());
            }
        }
    }
    set.into_iter().collect()
}

fn filter_sort_paginate_series(
    mut items: Vec<transcoderr_api_types::SeriesSummary>,
    params: &BrowseParams,
    available_codecs: Vec<String>,
    available_resolutions: Vec<String>,
) -> transcoderr_api_types::SeriesPage {
    if let Some(q) = params.search.as_ref().filter(|s| !s.is_empty()) {
        let needle = q.to_lowercase();
        items.retain(|s| s.title.to_lowercase().contains(&needle));
    }
    if let Some(c) = params.codec.as_ref().filter(|s| !s.is_empty()) {
        let needle = c.to_lowercase();
        items.retain(|s| s.codecs.iter().any(|x| x.to_lowercase() == needle));
    }
    if let Some(r) = params.resolution.as_ref().filter(|s| !s.is_empty()) {
        let needle = r.to_lowercase();
        items.retain(|s| s.resolutions.iter().any(|x| x.to_lowercase() == needle));
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
    transcoderr_api_types::SeriesPage {
        items: window,
        total,
        page,
        limit,
        available_codecs,
        available_resolutions,
    }
}

pub async fn series_get(
    State(state): State<AppState>,
    Path((source_id, series_id)): Path<(i64, i64)>,
) -> Result<Json<transcoderr_api_types::SeriesDetail>, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    if kind != arr::Kind::Sonarr {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.wrong_kind",
                "this endpoint is only for sonarr sources",
            )),
        ));
    }
    let key = format!("series:{series_id}");
    if let Some(v) = state.arr_cache.get(source_id, &key) {
        if let Ok(detail) = serde_json::from_value::<transcoderr_api_types::SeriesDetail>(v) {
            return Ok(Json(detail));
        }
    }
    let client = arr::Client::new(&base_url, &api_key)
        .map_err(|e| arr_call_error(source_id, e))?;
    let raw = client
        .get_series(series_id)
        .await
        .map_err(|e| arr_call_error(source_id, e))?;
    let detail = raw.into_detail(&base_url);
    let v = serde_json::to_value(&detail).unwrap_or(serde_json::Value::Null);
    state.arr_cache.put(source_id, &key, v);
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
pub struct EpisodesParams {
    #[serde(default)]
    pub season: Option<i32>,
    #[serde(default)]
    pub codec: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
}

pub async fn episodes(
    State(state): State<AppState>,
    Path((source_id, series_id)): Path<(i64, i64)>,
    Query(params): Query<EpisodesParams>,
) -> Result<Json<transcoderr_api_types::EpisodesPage>, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    if kind != arr::Kind::Sonarr {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError::new(
                "source.wrong_kind",
                "this endpoint is only for sonarr sources",
            )),
        ));
    }
    let key = format!("episodes:{series_id}");
    let trimmed: Vec<transcoderr_api_types::EpisodeSummary> =
        if let Some(v) = state.arr_cache.get(source_id, &key) {
            serde_json::from_value(v).unwrap_or_default()
        } else {
            let client = arr::Client::new(&base_url, &api_key)
                .map_err(|e| arr_call_error(source_id, e))?;
            let raw = client
                .list_episodes(series_id)
                .await
                .map_err(|e| arr_call_error(source_id, e))?;
            // Drop episodes the *arr knows about but hasn't imported a
            // file for (unaired, missing, etc.) — they're not transcodable.
            let trimmed: Vec<_> = raw
                .into_iter()
                .map(|e| e.into_summary())
                .filter(|e| e.has_file)
                .collect();
            let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
            state.arr_cache.put(source_id, &key, v);
            trimmed
        };

    // Compute available_* across the whole series (all seasons),
    // not just the current season filter — keeps dropdowns stable.
    let available_codecs = distinct_codecs(trimmed.iter(), |e| {
        e.file.as_ref().and_then(|f| f.codec.clone())
    });
    let available_resolutions = distinct_codecs(trimmed.iter(), |e| {
        e.file.as_ref().and_then(|f| f.resolution.clone())
    });

    let mut items: Vec<_> = trimmed.into_iter().collect();
    if let Some(s) = params.season {
        items.retain(|e| e.season_number == s);
    }
    if let Some(c) = params.codec.as_ref().filter(|s| !s.is_empty()) {
        let needle = c.to_lowercase();
        items.retain(|e| {
            e.file
                .as_ref()
                .and_then(|f| f.codec.as_ref())
                .map(|x| x.to_lowercase() == needle)
                .unwrap_or(false)
        });
    }
    if let Some(r) = params.resolution.as_ref().filter(|s| !s.is_empty()) {
        let needle = r.to_lowercase();
        items.retain(|e| {
            e.file
                .as_ref()
                .and_then(|f| f.resolution.as_ref())
                .map(|x| x.to_lowercase() == needle)
                .unwrap_or(false)
        });
    }
    items.sort_by(|a, b| {
        a.season_number
            .cmp(&b.season_number)
            .then_with(|| a.episode_number.cmp(&b.episode_number))
    });
    Ok(Json(transcoderr_api_types::EpisodesPage {
        items,
        available_codecs,
        available_resolutions,
    }))
}

pub async fn refresh(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    state.arr_cache.invalidate(source_id);

    // Warm the primary list endpoint for this kind. Best-effort: log
    // and ignore on failure — caller will see fresh data on next read.
    let client = match arr::Client::new(&base_url, &api_key) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(source_id, error = %e, "refresh: failed to build client; cache cleared anyway");
            return Ok(StatusCode::NO_CONTENT);
        }
    };
    match kind {
        arr::Kind::Radarr => {
            if let Ok(raw) = client.list_movies().await {
                let trimmed: Vec<_> = raw
                    .into_iter()
                    .map(|m| m.into_summary(&base_url))
                    .filter(|m| m.has_file)
                    .collect();
                let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
                state.arr_cache.put(source_id, CACHE_KEY_MOVIES, v);
            }
        }
        arr::Kind::Sonarr => {
            if let Ok(raw) = client.list_series().await {
                let trimmed: Vec<_> = raw
                    .into_iter()
                    .map(|s| s.into_summary(&base_url))
                    .filter(|s| s.episode_file_count > 0)
                    .collect();
                let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
                state.arr_cache.put(source_id, CACHE_KEY_SERIES, v);
            }
        }
        arr::Kind::Lidarr => {} // not browseable in v1
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn transcode(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
    Json(req): Json<TranscodeReq>,
) -> Result<Json<TranscodeResp>, (StatusCode, Json<ApiError>)> {
    let (row, kind, _base_url, _api_key) = browseable_source(&state, source_id).await?;

    let flows = db::flows::list_enabled_for_kind(&state.pool, kind)
        .await
        .map_err(|e| {
            tracing::error!(source_id, error = ?e, "transcode: failed to list enabled flows");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("db.error", "failed to list flows")),
            )
        })?;
    if flows.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            Json(ApiError::new(
                "no_enabled_flows",
                &format!("no enabled flows match kind {:?}", kind),
            )),
        ));
    }

    let payload = synthesize_payload(kind, &req);
    let payload_str = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());

    let mut runs = Vec::with_capacity(flows.len());
    for f in flows {
        match db::jobs::insert_with_source(
            &state.pool,
            f.id,
            f.version,
            row.id,
            &row.kind,
            &req.file_path,
            &payload_str,
        )
        .await
        {
            Ok(run_id) => runs.push(TranscodeRunRef {
                flow_id: f.id,
                flow_name: f.name,
                run_id,
            }),
            Err(e) => {
                tracing::error!(source_id, flow_id = f.id, error = ?e, "transcode: failed to insert job");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError::new("db.error", "failed to enqueue job")),
                ));
            }
        }
    }

    state.arr_cache.invalidate(source_id);
    tracing::info!(source_id, runs = runs.len(), file_path = %req.file_path, "manual transcode enqueued");
    Ok(Json(TranscodeResp { runs }))
}

fn synthesize_payload(kind: arr::Kind, req: &TranscodeReq) -> serde_json::Value {
    match kind {
        arr::Kind::Radarr => serde_json::json!({
            "eventType": "Manual",
            "movie": { "id": req.movie_id, "title": req.title },
            "movieFile": { "path": req.file_path },
            "_transcoderr_manual": true,
        }),
        arr::Kind::Sonarr => serde_json::json!({
            "eventType": "Manual",
            "series": { "id": req.series_id, "title": req.title },
            "episodes": [{ "id": req.episode_id }],
            "episodeFile": { "path": req.file_path },
            "_transcoderr_manual": true,
        }),
        arr::Kind::Lidarr => serde_json::json!({
            "eventType": "Manual",
            "_transcoderr_manual": true,
            "title": req.title,
            "path": req.file_path,
        }),
    }
}
