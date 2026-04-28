use crate::Server;
use rmcp::{
    handler::server::{tool::Parameters, wrapper::Json},
    model::ErrorData,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::future::Future; // pulled in by #[tool_router] macro expansion
use transcoderr_api_types::{
    EpisodesPage, MoviesPage, SeriesDetail, SeriesPage, TranscodeReq, TranscodeResp,
};

/// Percent-encode anything outside the unreserved set (RFC 3986).
/// Codec / resolution / sort values are usually clean; this exists for
/// `search` strings that might contain spaces or punctuation.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Builds a `?key=value&key=value...` query string, skipping null /
/// empty values. Returns `""` if every field was empty.
fn qs<I, K, V>(pairs: I) -> String
where
    I: IntoIterator<Item = (K, Option<V>)>,
    K: AsRef<str>,
    V: ToString,
{
    let parts: Vec<String> = pairs
        .into_iter()
        .filter_map(|(k, v)| {
            v.and_then(|val| {
                let s = val.to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(format!("{}={}", k.as_ref(), percent_encode(&s)))
                }
            })
        })
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("?{}", parts.join("&"))
    }
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ListMoviesArgs {
    /// Source id (radarr-kind, auto-provisioned). See list_sources.
    pub source_id: i64,
    /// Substring search on title (case-insensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Sort key: "title" (default) or "year".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// Filter to a specific codec (e.g. "h264", "x265"). The response's
    /// `available_codecs` array lists what's present in this library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// Filter to a specific resolution (e.g. "3840x2160"). The response's
    /// `available_resolutions` array lists what's present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    /// 1-indexed page; default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    /// Items per page; default 48, max 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ListSeriesArgs {
    pub source_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct GetSeriesArgs {
    pub source_id: i64,
    pub series_id: i64,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct ListEpisodesArgs {
    pub source_id: i64,
    pub series_id: i64,
    /// Optional: filter to a single season number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(Deserialize, Serialize, JsonSchema)]
pub struct TranscodeFileArgs {
    pub source_id: i64,
    /// Filesystem path to the file to transcode (use `file.path` from
    /// list_movies/list_episodes responses).
    pub file_path: String,
    /// Display name for logs / synthesized payload.
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<i64>,
}

#[tool_router(router = browse_router, vis = "pub")]
impl Server {
    #[tool(
        name = "list_movies",
        description = "List the auto-provisioned Radarr source's library, filtered to downloaded files only. Supports codec/resolution/search/sort + pagination. The response carries `available_codecs` and `available_resolutions` arrays — use them to discover what filter values are valid before calling again. For bulk operations (e.g. \"transcode every h264 movie\"), set limit=200 and iterate page until items.length < limit."
    )]
    pub async fn list_movies(
        &self,
        Parameters(a): Parameters<ListMoviesArgs>,
    ) -> Result<Json<MoviesPage>, ErrorData> {
        let q = qs([
            ("search", a.search),
            ("sort", a.sort),
            ("codec", a.codec),
            ("resolution", a.resolution),
            ("page", a.page.map(|n| n.to_string())),
            ("limit", a.limit.map(|n| n.to_string())),
        ]);
        self.api
            .get::<MoviesPage>(&format!("/api/sources/{}/movies{}", a.source_id, q))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "list_series",
        description = "List the auto-provisioned Sonarr source's series, filtered to series with at least one downloaded episode. Use list_episodes to drill into a specific series."
    )]
    pub async fn list_series(
        &self,
        Parameters(a): Parameters<ListSeriesArgs>,
    ) -> Result<Json<SeriesPage>, ErrorData> {
        let q = qs([
            ("search", a.search),
            ("sort", a.sort),
            ("page", a.page.map(|n| n.to_string())),
            ("limit", a.limit.map(|n| n.to_string())),
        ]);
        self.api
            .get::<SeriesPage>(&format!("/api/sources/{}/series{}", a.source_id, q))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "get_series",
        description = "Fetch full Sonarr series detail: poster, fanart, overview, season-level episode counts. Use list_episodes for the actual episode rows."
    )]
    pub async fn get_series(
        &self,
        Parameters(a): Parameters<GetSeriesArgs>,
    ) -> Result<Json<SeriesDetail>, ErrorData> {
        self.api
            .get::<SeriesDetail>(&format!(
                "/api/sources/{}/series/{}",
                a.source_id, a.series_id
            ))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "list_episodes",
        description = "List downloaded episodes of a Sonarr series. Filter by season/codec/resolution. Response includes `available_codecs` and `available_resolutions` across the whole series (not just the active season filter), so picking values is one round-trip. For bulk operations like \"re-encode every 1080p episode of this show\", iterate the items and call transcode_file."
    )]
    pub async fn list_episodes(
        &self,
        Parameters(a): Parameters<ListEpisodesArgs>,
    ) -> Result<Json<EpisodesPage>, ErrorData> {
        let q = qs([
            ("season", a.season.map(|n| n.to_string())),
            ("codec", a.codec),
            ("resolution", a.resolution),
        ]);
        self.api
            .get::<EpisodesPage>(&format!(
                "/api/sources/{}/series/{}/episodes{}",
                a.source_id, a.series_id, q
            ))
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }

    #[tool(
        name = "transcode_file",
        description = "Side effect: enqueue transcode runs for a specific file. Fans out across every enabled flow that handles the source's kind (same semantics as a real *arr push). Returns the new run ids; check progress via list_runs/get_run. Use for both single-file and bulk-mode workflows — for bulk, list_movies/list_episodes first, then call this per item."
    )]
    pub async fn transcode_file(
        &self,
        Parameters(a): Parameters<TranscodeFileArgs>,
    ) -> Result<Json<TranscodeResp>, ErrorData> {
        let body = TranscodeReq {
            file_path: a.file_path,
            title: a.title,
            movie_id: a.movie_id,
            series_id: a.series_id,
            episode_id: a.episode_id,
        };
        self.api
            .post::<TranscodeResp, _>(&format!("/api/sources/{}/transcode", a.source_id), &body)
            .await
            .map(Json)
            .map_err(|e| e.into_error_data())
    }
}
