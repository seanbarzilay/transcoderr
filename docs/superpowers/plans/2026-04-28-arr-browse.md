# Radarr / Sonarr Browse-and-Transcode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two new WebUI pages — `/radarr` and `/sonarr` — that let an operator browse the auto-provisioned *arr's library and manually enqueue a transcode for a chosen movie/episode. Bypasses the webhook trigger; same fan-out semantics.

**Architecture:** A bespoke `/api/sources/:id/{movies,series,...}` proxy layer in transcoderr calls the *arr's REST API with the stored credentials, trims responses to UI-only fields, and serves them through a 5-min in-memory TTL cache. Posters render in the browser straight from TMDB/TheTVDB CDNs (no image-byte proxy). `POST /transcode` synthesizes a webhook-shaped payload and fans out across all enabled flows for the source's kind.

**Tech Stack:** Rust (axum, sqlx, reqwest, anyhow, tokio, tracing, wiremock), React + TypeScript + react-query + react-router-dom.

**Spec:** `docs/superpowers/specs/2026-04-28-arr-browse-design.md`

---

## File Structure

```
crates/transcoderr-api-types/src/lib.rs                   [modify: add MovieSummary/SeriesSummary/SeriesDetail/SeasonSummary/EpisodeSummary/FileSummary/MoviesPage/SeriesPage/EpisodesPage/TranscodeReq/TranscodeResp]

crates/transcoderr/src/arr/cache.rs                       [create: ArrCache (TTL'd HashMap), 3 unit tests]
crates/transcoderr/src/arr/browse.rs                      [create: RadarrMovie/SonarrSeries/SonarrEpisode/SonarrEpisodeFile/MediaInfo deserialize structs + From-impls into trimmed types + unit tests]
crates/transcoderr/src/arr/mod.rs                         [modify: pub mod {browse, cache}; add list_movies/list_series/get_series/list_episodes to Client + 4 wiremock tests]

crates/transcoderr/src/db/flows.rs                        [modify: add list_enabled_for_kind helper]

crates/transcoderr/src/http/mod.rs                        [modify: AppState gains arr_cache field]
crates/transcoderr/src/main.rs                            [modify: build ArrCache, inject into AppState]
crates/transcoderr/tests/common/mod.rs                    [modify: add arr_cache to test boot AppState literal]

crates/transcoderr/src/api/arr_browse.rs                  [create: shared validation + 6 handlers (movies, series, series_get, episodes, transcode, refresh)]
crates/transcoderr/src/api/mod.rs                         [modify: route the 6 new endpoints]
crates/transcoderr/tests/arr_browse.rs                    [create: wiremock-backed integration tests]

web/src/types-arr.ts                                      [create: TS mirrors of the Rust trimmed shapes]
web/src/api/client.ts                                     [modify: add arr.{movies, series, seriesGet, episodes, transcode, refresh}]

web/src/components/source-picker.tsx                      [create]
web/src/components/poster-grid.tsx                        [create]
web/src/components/detail-panel.tsx                       [create]
web/src/components/transcode-button.tsx                   [create]

web/src/pages/radarr.tsx                                  [create]
web/src/pages/sonarr.tsx                                  [create]
web/src/pages/sonarr-series.tsx                           [create]

web/src/components/sidebar.tsx                            [modify: two new nav-links under Operate]
web/src/App.tsx                                           [modify: three new routes]
web/src/index.css                                         [modify: poster-grid + detail-panel + season-tabs styles]
```

---

## Task 1: API types in `transcoderr-api-types`

**Files:**
- Modify: `crates/transcoderr-api-types/src/lib.rs` (append types at bottom)

These are pure data structures with `Serialize/Deserialize/JsonSchema` derives. No logic, no tests at this layer (they're exercised end-to-end in Task 11's integration tests).

- [ ] **Step 1: Append the type definitions**

Open `crates/transcoderr-api-types/src/lib.rs`. After the last `}` in the file, append:

```rust
// ─── *arr browse ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FileSummary {
    pub path: String,
    pub size: i64,
    pub codec: Option<String>,
    pub quality: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MovieSummary {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub has_file: bool,
    pub file: Option<FileSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SeriesSummary {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub season_count: i32,
    pub episode_count: i32,
    pub episode_file_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SeasonSummary {
    pub number: i32,
    pub episode_count: i32,
    pub episode_file_count: i32,
    pub monitored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SeriesDetail {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub fanart_url: Option<String>,
    pub seasons: Vec<SeasonSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EpisodeSummary {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: String,
    pub air_date: Option<String>,
    pub has_file: bool,
    pub file: Option<FileSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoviesPage {
    pub items: Vec<MovieSummary>,
    pub total: i64,
    pub page: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SeriesPage {
    pub items: Vec<SeriesSummary>,
    pub total: i64,
    pub page: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EpisodesPage {
    pub items: Vec<EpisodeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeReq {
    pub file_path: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movie_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeRunRef {
    pub flow_id: i64,
    pub flow_name: String,
    pub run_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscodeResp {
    pub runs: Vec<TranscodeRunRef>,
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p transcoderr-api-types 2>&1 | tail -3`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr-api-types/src/lib.rs
git commit -m "feat(api-types): browse + transcode response shapes"
```

---

## Task 2: `arr::cache` module

**Files:**
- Create: `crates/transcoderr/src/arr/cache.rs`

Standalone TTL'd cache with no HTTP/DB dependencies. Tests use a mocked clock (closure returning `Instant`) so we don't sleep.

- [ ] **Step 1: Write the failing test**

Create `crates/transcoderr/src/arr/cache.rs`:

```rust
//! In-memory TTL cache for trimmed *arr browse responses. Stored on
//! `AppState` as `Arc<ArrCache>`; cache holds the full library so that
//! search/sort/pagination on hits are sub-millisecond.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct ArrCache {
    inner: Arc<RwLock<HashMap<(i64, String), CacheEntry>>>,
    ttl: Duration,
    now_fn: Arc<dyn Fn() -> Instant + Send + Sync>,
}

struct CacheEntry {
    data: serde_json::Value,
    expires_at: Instant,
}

impl ArrCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
            now_fn: Arc::new(Instant::now),
        }
    }

    /// Test-only constructor with an injectable clock.
    #[cfg(test)]
    pub fn new_with_clock(ttl: Duration, now_fn: Arc<dyn Fn() -> Instant + Send + Sync>) -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())), ttl, now_fn }
    }

    pub async fn get(&self, source_id: i64, key: &str) -> Option<serde_json::Value> {
        let now = (self.now_fn)();
        let g = self.inner.read().await;
        let e = g.get(&(source_id, key.to_string()))?;
        if e.expires_at <= now { return None; }
        Some(e.data.clone())
    }

    pub async fn put(&self, source_id: i64, key: &str, data: serde_json::Value) {
        let expires_at = (self.now_fn)() + self.ttl;
        let mut g = self.inner.write().await;
        g.insert((source_id, key.to_string()), CacheEntry { data, expires_at });
    }

    /// Drops every entry whose source_id matches.
    pub async fn invalidate(&self, source_id: i64) {
        let mut g = self.inner.write().await;
        g.retain(|(sid, _), _| *sid != source_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn fake_clock(start: Instant) -> (Arc<dyn Fn() -> Instant + Send + Sync>, Arc<Mutex<Instant>>) {
        let now = Arc::new(Mutex::new(start));
        let now_fn_handle = now.clone();
        let now_fn: Arc<dyn Fn() -> Instant + Send + Sync> =
            Arc::new(move || *now_fn_handle.lock().unwrap());
        (now_fn, now)
    }

    #[tokio::test]
    async fn cache_returns_value_within_ttl() {
        let (clock, now_handle) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([{"id": 42}])).await;
        // Advance clock by 4 minutes — still within 5-minute TTL.
        *now_handle.lock().unwrap() += Duration::from_secs(240);
        let got = c.get(1, "movies").await.unwrap();
        assert_eq!(got, serde_json::json!([{"id": 42}]));
    }

    #[tokio::test]
    async fn cache_returns_none_after_ttl_expiry() {
        let (clock, now_handle) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([{"id": 42}])).await;
        // Advance past the 5-minute TTL.
        *now_handle.lock().unwrap() += Duration::from_secs(301);
        assert!(c.get(1, "movies").await.is_none());
    }

    #[tokio::test]
    async fn invalidate_drops_all_keys_for_source_id() {
        let (clock, _) = fake_clock(Instant::now());
        let c = ArrCache::new_with_clock(Duration::from_secs(300), clock);
        c.put(1, "movies", serde_json::json!([1])).await;
        c.put(1, "series", serde_json::json!([2])).await;
        c.put(2, "movies", serde_json::json!([3])).await;
        c.invalidate(1).await;
        assert!(c.get(1, "movies").await.is_none());
        assert!(c.get(1, "series").await.is_none());
        assert!(c.get(2, "movies").await.is_some()); // unaffected
    }
}
```

- [ ] **Step 2: Wire the module into `arr/mod.rs`**

In `crates/transcoderr/src/arr/mod.rs`, find the existing `pub mod reconcile;` line near the top. Add immediately after it:

```rust
pub mod cache;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib arr::cache:: 2>&1 | tail -10`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/arr/cache.rs crates/transcoderr/src/arr/mod.rs
git commit -m "feat(arr): TTL cache for browse-proxy responses"
```

---

## Task 3: `arr::browse` module — types + trim impls

**Files:**
- Create: `crates/transcoderr/src/arr/browse.rs`
- Modify: `crates/transcoderr/src/arr/mod.rs` (add `pub mod browse;`)

The `browse.rs` file owns the *arr REST schema knowledge: `RadarrMovie`, `SonarrSeries`, `SonarrEpisode`, `SonarrEpisodeFile`, plus `From` impls to the trimmed `transcoderr_api_types::*Summary` shapes.

- [ ] **Step 1: Create the module**

Create `crates/transcoderr/src/arr/browse.rs`:

```rust
//! Schema definitions for the *arr REST endpoints we browse, plus
//! `From` impls that trim them down to the wire-stable summary types
//! in `transcoderr_api_types`. All *arr-schema knowledge lives here so
//! the rest of the codebase doesn't bind to *arr API details.

use serde::Deserialize;
use transcoderr_api_types::{
    EpisodeSummary, FileSummary, MovieSummary, SeasonSummary, SeriesDetail, SeriesSummary,
};

/// Single image entry from an *arr response.
#[derive(Debug, Deserialize)]
pub struct ArrImage {
    #[serde(rename = "coverType", default)]
    pub cover_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "remoteUrl", default)]
    pub remote_url: String,
}

/// Pick `remoteUrl` if non-empty, else fall back to `base_url + url`.
fn pick_image_url(images: &[ArrImage], cover_type: &str, base_url: &str) -> Option<String> {
    let img = images.iter().find(|i| i.cover_type == cover_type)?;
    if !img.remote_url.is_empty() {
        return Some(img.remote_url.clone());
    }
    if !img.url.is_empty() {
        return Some(format!("{}{}", base_url.trim_end_matches('/'), img.url));
    }
    None
}

#[derive(Debug, Deserialize)]
pub struct RadarrMediaInfo {
    #[serde(rename = "videoCodec", default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrQualityName {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrQuality {
    #[serde(default)]
    pub quality: Option<RadarrQualityName>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovieFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "mediaInfo", default)]
    pub media_info: Option<RadarrMediaInfo>,
    #[serde(default)]
    pub quality: Option<RadarrQuality>,
}

#[derive(Debug, Deserialize)]
pub struct RadarrMovie {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub images: Vec<ArrImage>,
    #[serde(rename = "hasFile", default)]
    pub has_file: bool,
    #[serde(rename = "movieFile", default)]
    pub movie_file: Option<RadarrMovieFile>,
}

impl RadarrMovie {
    pub fn into_summary(self, base_url: &str) -> MovieSummary {
        let poster_url = pick_image_url(&self.images, "poster", base_url);
        let file = if self.has_file {
            self.movie_file.as_ref().map(|f| FileSummary {
                path: f.path.clone(),
                size: f.size,
                codec: f.media_info.as_ref().and_then(|m| m.video_codec.clone()),
                quality: f.quality.as_ref().and_then(|q| q.quality.as_ref()).and_then(|n| n.name.clone()),
                resolution: f.media_info.as_ref().and_then(|m| m.resolution.clone()),
            })
        } else {
            None
        };
        MovieSummary {
            id: self.id,
            title: self.title,
            year: self.year,
            poster_url,
            has_file: self.has_file,
            file,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeasonStatistics {
    #[serde(rename = "episodeCount", default)]
    pub episode_count: i32,
    #[serde(rename = "episodeFileCount", default)]
    pub episode_file_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeason {
    #[serde(rename = "seasonNumber")]
    pub season_number: i32,
    #[serde(default)]
    pub monitored: bool,
    #[serde(default)]
    pub statistics: Option<SonarrSeasonStatistics>,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeriesStatistics {
    #[serde(rename = "seasonCount", default)]
    pub season_count: i32,
    #[serde(rename = "episodeCount", default)]
    pub episode_count: i32,
    #[serde(rename = "episodeFileCount", default)]
    pub episode_file_count: i32,
}

#[derive(Debug, Deserialize)]
pub struct SonarrSeries {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub images: Vec<ArrImage>,
    #[serde(default)]
    pub seasons: Vec<SonarrSeason>,
    #[serde(default)]
    pub statistics: Option<SonarrSeriesStatistics>,
}

impl SonarrSeries {
    pub fn into_summary(self, base_url: &str) -> SeriesSummary {
        let stats = self.statistics.as_ref();
        SeriesSummary {
            id: self.id,
            title: self.title,
            year: self.year,
            poster_url: pick_image_url(&self.images, "poster", base_url),
            season_count: stats.map(|s| s.season_count).unwrap_or(0),
            episode_count: stats.map(|s| s.episode_count).unwrap_or(0),
            episode_file_count: stats.map(|s| s.episode_file_count).unwrap_or(0),
        }
    }

    pub fn into_detail(self, base_url: &str) -> SeriesDetail {
        let poster_url = pick_image_url(&self.images, "poster", base_url);
        let fanart_url = pick_image_url(&self.images, "fanart", base_url);
        let seasons = self
            .seasons
            .iter()
            .map(|s| SeasonSummary {
                number: s.season_number,
                episode_count: s.statistics.as_ref().map(|x| x.episode_count).unwrap_or(0),
                episode_file_count: s.statistics.as_ref().map(|x| x.episode_file_count).unwrap_or(0),
                monitored: s.monitored,
            })
            .collect();
        SeriesDetail {
            id: self.id,
            title: self.title,
            year: self.year,
            overview: self.overview,
            poster_url,
            fanart_url,
            seasons,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SonarrEpisodeFile {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(rename = "mediaInfo", default)]
    pub media_info: Option<RadarrMediaInfo>,
    #[serde(default)]
    pub quality: Option<RadarrQuality>,
}

#[derive(Debug, Deserialize)]
pub struct SonarrEpisode {
    pub id: i64,
    #[serde(rename = "seasonNumber")]
    pub season_number: i32,
    #[serde(rename = "episodeNumber")]
    pub episode_number: i32,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "airDate", default)]
    pub air_date: Option<String>,
    #[serde(rename = "hasFile", default)]
    pub has_file: bool,
    #[serde(rename = "episodeFile", default)]
    pub episode_file: Option<SonarrEpisodeFile>,
}

impl SonarrEpisode {
    pub fn into_summary(self) -> EpisodeSummary {
        let file = if self.has_file {
            self.episode_file.as_ref().map(|f| FileSummary {
                path: f.path.clone(),
                size: f.size,
                codec: f.media_info.as_ref().and_then(|m| m.video_codec.clone()),
                quality: f.quality.as_ref().and_then(|q| q.quality.as_ref()).and_then(|n| n.name.clone()),
                resolution: f.media_info.as_ref().and_then(|m| m.resolution.clone()),
            })
        } else {
            None
        };
        EpisodeSummary {
            id: self.id,
            season_number: self.season_number,
            episode_number: self.episode_number,
            title: self.title,
            air_date: self.air_date,
            has_file: self.has_file,
            file,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn radarr_movie_trims_to_summary_with_file() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 7,
            "title": "Dune",
            "year": 2021,
            "hasFile": true,
            "images": [
                { "coverType": "poster", "remoteUrl": "https://image.tmdb.org/x.jpg", "url": "/MediaCover/7/poster.jpg" },
                { "coverType": "fanart", "remoteUrl": "https://image.tmdb.org/y.jpg", "url": "" }
            ],
            "movieFile": {
                "path": "/movies/Dune.mkv",
                "size": 42_000_000_000_i64,
                "mediaInfo": { "videoCodec": "x265", "resolution": "3840x2160" },
                "quality": { "quality": { "name": "Bluray-2160p" } }
            }
        })).unwrap();
        let s = raw.into_summary("http://radarr:7878");
        assert_eq!(s.id, 7);
        assert_eq!(s.title, "Dune");
        assert_eq!(s.poster_url.as_deref(), Some("https://image.tmdb.org/x.jpg"));
        assert!(s.has_file);
        let f = s.file.unwrap();
        assert_eq!(f.path, "/movies/Dune.mkv");
        assert_eq!(f.size, 42_000_000_000_i64);
        assert_eq!(f.codec.as_deref(), Some("x265"));
        assert_eq!(f.resolution.as_deref(), Some("3840x2160"));
        assert_eq!(f.quality.as_deref(), Some("Bluray-2160p"));
    }

    #[test]
    fn radarr_movie_no_file_omits_file_summary() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 8, "title": "Pending", "hasFile": false, "images": []
        })).unwrap();
        let s = raw.into_summary("http://radarr:7878");
        assert!(!s.has_file);
        assert!(s.file.is_none());
    }

    #[test]
    fn radarr_movie_falls_back_to_local_url_when_remote_empty() {
        let raw: RadarrMovie = serde_json::from_value(json!({
            "id": 9, "title": "Local", "hasFile": false,
            "images": [{ "coverType": "poster", "remoteUrl": "", "url": "/MediaCover/9/poster.jpg" }]
        })).unwrap();
        let s = raw.into_summary("http://radarr:7878/");
        assert_eq!(s.poster_url.as_deref(), Some("http://radarr:7878/MediaCover/9/poster.jpg"));
    }

    #[test]
    fn sonarr_series_trims_to_summary() {
        let raw: SonarrSeries = serde_json::from_value(json!({
            "id": 1, "title": "Foundation", "year": 2021,
            "images": [{ "coverType": "poster", "remoteUrl": "https://artworks.thetvdb.com/p.jpg" }],
            "statistics": { "seasonCount": 3, "episodeCount": 30, "episodeFileCount": 25 }
        })).unwrap();
        let s = raw.into_summary("http://sonarr:8989");
        assert_eq!(s.id, 1);
        assert_eq!(s.season_count, 3);
        assert_eq!(s.episode_count, 30);
        assert_eq!(s.episode_file_count, 25);
        assert_eq!(s.poster_url.as_deref(), Some("https://artworks.thetvdb.com/p.jpg"));
    }

    #[test]
    fn sonarr_series_into_detail_carries_seasons_and_fanart() {
        let raw: SonarrSeries = serde_json::from_value(json!({
            "id": 1, "title": "Foundation",
            "overview": "Galactic decline",
            "images": [
                { "coverType": "poster", "remoteUrl": "https://p" },
                { "coverType": "fanart", "remoteUrl": "https://f" }
            ],
            "seasons": [
                { "seasonNumber": 1, "monitored": true,
                  "statistics": { "episodeCount": 10, "episodeFileCount": 10 } },
                { "seasonNumber": 2, "monitored": false,
                  "statistics": { "episodeCount": 10, "episodeFileCount": 5 } }
            ]
        })).unwrap();
        let d = raw.into_detail("http://sonarr:8989");
        assert_eq!(d.fanart_url.as_deref(), Some("https://f"));
        assert_eq!(d.seasons.len(), 2);
        assert_eq!(d.seasons[1].number, 2);
        assert!(!d.seasons[1].monitored);
        assert_eq!(d.seasons[1].episode_file_count, 5);
    }

    #[test]
    fn sonarr_episode_trims_to_summary() {
        let raw: SonarrEpisode = serde_json::from_value(json!({
            "id": 100, "seasonNumber": 1, "episodeNumber": 3,
            "title": "Pilot", "airDate": "2021-09-24", "hasFile": true,
            "episodeFile": {
                "path": "/tv/Foundation/S01E03.mkv", "size": 5_000_000_000_i64,
                "mediaInfo": { "videoCodec": "h264", "resolution": "1920x1080" },
                "quality": { "quality": { "name": "WEBDL-1080p" } }
            }
        })).unwrap();
        let s = raw.into_summary();
        assert_eq!(s.id, 100);
        assert_eq!(s.season_number, 1);
        assert_eq!(s.episode_number, 3);
        assert_eq!(s.title, "Pilot");
        assert_eq!(s.air_date.as_deref(), Some("2021-09-24"));
        let f = s.file.unwrap();
        assert_eq!(f.codec.as_deref(), Some("h264"));
        assert_eq!(f.resolution.as_deref(), Some("1920x1080"));
        assert_eq!(f.quality.as_deref(), Some("WEBDL-1080p"));
    }
}
```

- [ ] **Step 2: Wire the module**

In `crates/transcoderr/src/arr/mod.rs`, immediately after the `pub mod cache;` line added in Task 2, add:

```rust
pub mod browse;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib arr::browse:: 2>&1 | tail -15`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/arr/browse.rs crates/transcoderr/src/arr/mod.rs
git commit -m "feat(arr): browse types + trim impls (movies/series/episodes)"
```

---

## Task 4: `arr::Client` GET methods + wiremock tests

**Files:**
- Modify: `crates/transcoderr/src/arr/mod.rs` (append 4 methods to `impl Client { ... }` and 4 tests inside `mod tests`)

Adds the four library-fetch methods. Each follows the existing GET pattern (X-Api-Key header, non-2xx → bail with status + body).

- [ ] **Step 1: Append the methods**

In `crates/transcoderr/src/arr/mod.rs`, find the closing `}` of `impl Client { ... }` (it's directly before the `event_flags` standalone function). Just before that closing `}`, append:

```rust
    pub async fn list_movies(&self) -> Result<Vec<crate::arr::browse::RadarrMovie>> {
        let url = format!("{}/api/v3/movie", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("listing radarr movies")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<Vec<crate::arr::browse::RadarrMovie>>()
            .await
            .context("parsing radarr movie list")
    }

    pub async fn list_series(&self) -> Result<Vec<crate::arr::browse::SonarrSeries>> {
        let url = format!("{}/api/v3/series", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("listing sonarr series")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<Vec<crate::arr::browse::SonarrSeries>>()
            .await
            .context("parsing sonarr series list")
    }

    pub async fn get_series(&self, id: i64) -> Result<crate::arr::browse::SonarrSeries> {
        let url = format!("{}/api/v3/series/{id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("getting sonarr series")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<crate::arr::browse::SonarrSeries>()
            .await
            .context("parsing sonarr series response")
    }

    pub async fn list_episodes(&self, series_id: i64) -> Result<Vec<crate::arr::browse::SonarrEpisode>> {
        let url = format!("{}/api/v3/episode?seriesId={series_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("X-Api-Key", &self.api_key)
            .send()
            .await
            .context("listing sonarr episodes")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("*arr returned {status}: {text}");
        }
        resp.json::<Vec<crate::arr::browse::SonarrEpisode>>()
            .await
            .context("parsing sonarr episode list")
    }
```

- [ ] **Step 2: Append wiremock tests inside `mod tests`**

In the same file, find the closing `}` of `mod tests` (very end of file) and just before it append:

```rust
    use wiremock::matchers::query_param;

    #[tokio::test]
    async fn list_movies_hits_correct_path_with_api_key() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/movie"))
            .and(header("X-Api-Key", "k"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 7, "title": "Dune", "year": 2021,
                  "hasFile": false, "images": [] }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let c = Client::new(&server.uri(), "k").unwrap();
        let movies = c.list_movies().await.unwrap();
        assert_eq!(movies.len(), 1);
        assert_eq!(movies[0].title, "Dune");
    }

    #[tokio::test]
    async fn list_series_hits_correct_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/series"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "title": "Foundation" }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let c = Client::new(&server.uri(), "k").unwrap();
        let series = c.list_series().await.unwrap();
        assert_eq!(series.len(), 1);
    }

    #[tokio::test]
    async fn get_series_includes_id_in_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/series/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42, "title": "Babylon 5"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let c = Client::new(&server.uri(), "k").unwrap();
        let s = c.get_series(42).await.unwrap();
        assert_eq!(s.id, 42);
    }

    #[tokio::test]
    async fn list_episodes_passes_series_id_query() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/episode"))
            .and(query_param("seriesId", "42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "id": 1, "seasonNumber": 1, "episodeNumber": 1, "title": "Pilot", "hasFile": false }
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let c = Client::new(&server.uri(), "k").unwrap();
        let eps = c.list_episodes(42).await.unwrap();
        assert_eq!(eps.len(), 1);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p transcoderr --lib arr:: 2>&1 | tail -10`
Expected: previous arr tests still pass plus 4 new ones.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/arr/mod.rs
git commit -m "feat(arr): Client::list_movies/list_series/get_series/list_episodes"
```

---

## Task 5: `db::flows::list_enabled_for_kind`

**Files:**
- Modify: `crates/transcoderr/src/db/flows.rs` (append helper)

Returns ALL enabled flows whose YAML wires any event for that source kind, regardless of event semantics. Used by `POST /transcode` for any-event manual fan-out.

- [ ] **Step 1: Append the helper**

In `crates/transcoderr/src/db/flows.rs`, after the existing `list_enabled_for_lidarr` function, append:

```rust
/// Returns all enabled flows whose YAML wires any event for the given
/// source kind. Used by the manual-trigger path (Browse Radarr/Sonarr
/// pages), which fans out across every matching flow regardless of
/// event semantics — the operator already chose to trigger.
pub async fn list_enabled_for_kind(
    pool: &SqlitePool,
    kind: crate::arr::Kind,
) -> anyhow::Result<Vec<FlowRow>> {
    let all = sqlx::query_as::<_, (i64, String, i64, String, String, i64)>(
        "SELECT id, name, enabled, yaml_source, parsed_json, version FROM flows WHERE enabled = 1"
    ).fetch_all(pool).await?;
    let mut out = vec![];
    for (id, name, enabled, yaml_source, parsed_json, version) in all {
        let flow: Flow = serde_json::from_str(&parsed_json)?;
        let matches = flow.triggers.iter().any(|t| match (kind, t) {
            (crate::arr::Kind::Radarr, crate::flow::Trigger::Radarr(events)) => !events.is_empty(),
            (crate::arr::Kind::Sonarr, crate::flow::Trigger::Sonarr(events)) => !events.is_empty(),
            (crate::arr::Kind::Lidarr, crate::flow::Trigger::Lidarr(events)) => !events.is_empty(),
            _ => false,
        });
        if matches {
            out.push(FlowRow { id, name, enabled: enabled != 0, yaml_source, parsed_json, version });
        }
    }
    Ok(out)
}
```

- [ ] **Step 2: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean build (no new tests at this layer; coverage comes from Task 11's transcode test).

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/db/flows.rs
git commit -m "feat(db): list_enabled_for_kind for any-event manual fan-out"
```

---

## Task 6: AppState plumbing for `arr_cache`

**Files:**
- Modify: `crates/transcoderr/src/http/mod.rs` (add field)
- Modify: `crates/transcoderr/src/main.rs` (build + inject)
- Modify: `crates/transcoderr/tests/common/mod.rs` (test boot helper)

- [ ] **Step 1: Add field to `AppState`**

In `crates/transcoderr/src/http/mod.rs`, find the `pub struct AppState { ... }`. Append a new field:

```rust
    pub arr_cache: std::sync::Arc<crate::arr::cache::ArrCache>,
```

- [ ] **Step 2: Build the cache in `serve`**

In `crates/transcoderr/src/main.rs`, find the line that constructs `state: AppState` (after Task 6 of source-autoprovision plumbed in `public_url`). Just before the `let state = transcoderr::http::AppState { ... };` line, add:

```rust
let arr_cache = std::sync::Arc::new(transcoderr::arr::cache::ArrCache::new(
    std::time::Duration::from_secs(300),
));
```

Then add `arr_cache,` to the `AppState { ... }` struct literal (the order doesn't matter — match the struct's field order or just append).

- [ ] **Step 3: Update test boot helper**

In `crates/transcoderr/tests/common/mod.rs`, find the `AppState { ... }` literal in `boot()`. Add:

```rust
    arr_cache: std::sync::Arc::new(transcoderr::arr::cache::ArrCache::new(
        std::time::Duration::from_secs(300),
    )),
```

- [ ] **Step 4: Build + run wider tests**

Run: `cargo build --workspace 2>&1 | tail -3`
Expected: clean.

Run: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -10`
Expected: all `ok` (the pre-existing `metrics_endpoint_responds_with_text_format` flake is acceptable).

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/http/mod.rs crates/transcoderr/src/main.rs crates/transcoderr/tests/common/mod.rs
git commit -m "feat(http): AppState gains arr_cache; built at serve()"
```

---

## Task 7: Shared validation + `GET /sources/:id/movies`

**Files:**
- Create: `crates/transcoderr/src/api/arr_browse.rs` (validation helper + first proxy handler)
- Modify: `crates/transcoderr/src/api/mod.rs` (declare module + route the endpoint)

The shared helper `browseable_source(state, id)` returns `Result<(SourceRow, arr::Kind, String /*base_url*/, String /*api_key*/), (StatusCode, Json<ApiError>)>`. All 5 GET proxy handlers and the transcode handler use it.

- [ ] **Step 1: Create the module with helper + first handler**

Create `crates/transcoderr/src/api/arr_browse.rs`:

```rust
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
fn arr_call_error(source_id: i64, e: anyhow::Error) -> (StatusCode, Json<ApiError>) {
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

const CACHE_KEY_MOVIES: &str = "movies";

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
    let cached = state.arr_cache.get(source_id, CACHE_KEY_MOVIES).await;
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
        state.arr_cache.put(source_id, CACHE_KEY_MOVIES, v).await;
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
```

- [ ] **Step 2: Declare the module + route the endpoint**

In `crates/transcoderr/src/api/mod.rs`, with the other `pub mod` lines at the top, add:

```rust
pub mod arr_browse;
```

In the same file, find the `protected` Router builder. Add this line in the same block where other `/sources/...` routes live (e.g. just after `.route("/sources/:id/test-fire", post(sources::test_fire))`):

```rust
        .route("/sources/:id/movies", get(arr_browse::movies))
```

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean (handler exists, route wired).

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/api/arr_browse.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): GET /sources/:id/movies proxy with cache+search+sort+paginate"
```

---

## Task 8: `GET /sources/:id/series` and `/series/:id`

**Files:**
- Modify: `crates/transcoderr/src/api/arr_browse.rs` (append handlers + helper)
- Modify: `crates/transcoderr/src/api/mod.rs` (route)

Two related handlers in one task — both serve sonarr series shapes; they share trim + cache patterns with `movies`.

- [ ] **Step 1: Append handlers**

In `crates/transcoderr/src/api/arr_browse.rs`, append at the bottom:

```rust
const CACHE_KEY_SERIES: &str = "series";

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
    let cached = state.arr_cache.get(source_id, CACHE_KEY_SERIES).await;
    let trimmed: Vec<transcoderr_api_types::SeriesSummary> = if let Some(v) = cached {
        serde_json::from_value(v).unwrap_or_default()
    } else {
        let client = arr::Client::new(&base_url, &api_key)
            .map_err(|e| arr_call_error(source_id, e))?;
        let raw = client
            .list_series()
            .await
            .map_err(|e| arr_call_error(source_id, e))?;
        let trimmed: Vec<_> = raw.into_iter().map(|s| s.into_summary(&base_url)).collect();
        let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
        state.arr_cache.put(source_id, CACHE_KEY_SERIES, v).await;
        trimmed
    };
    Ok(Json(filter_sort_paginate_series(trimmed, &params)))
}

fn filter_sort_paginate_series(
    mut items: Vec<transcoderr_api_types::SeriesSummary>,
    params: &BrowseParams,
) -> transcoderr_api_types::SeriesPage {
    if let Some(q) = params.search.as_ref().filter(|s| !s.is_empty()) {
        let needle = q.to_lowercase();
        items.retain(|s| s.title.to_lowercase().contains(&needle));
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
    transcoderr_api_types::SeriesPage { items: window, total, page, limit }
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
    if let Some(v) = state.arr_cache.get(source_id, &key).await {
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
    state.arr_cache.put(source_id, &key, v).await;
    Ok(Json(detail))
}
```

- [ ] **Step 2: Route both endpoints**

In `crates/transcoderr/src/api/mod.rs`, immediately after the `.route("/sources/:id/movies", ...)` line added in Task 7, append:

```rust
        .route("/sources/:id/series", get(arr_browse::series))
        .route("/sources/:id/series/:series_id", get(arr_browse::series_get))
```

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/api/arr_browse.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): GET /sources/:id/series and /series/:id"
```

---

## Task 9: `GET /sources/:id/series/:series_id/episodes`

**Files:**
- Modify: `crates/transcoderr/src/api/arr_browse.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

Episodes endpoint with optional `?season=` filter applied server-side.

- [ ] **Step 1: Append handler**

In `crates/transcoderr/src/api/arr_browse.rs`, append at the bottom:

```rust
#[derive(Debug, Deserialize)]
pub struct EpisodesParams {
    #[serde(default)]
    pub season: Option<i32>,
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
        if let Some(v) = state.arr_cache.get(source_id, &key).await {
            serde_json::from_value(v).unwrap_or_default()
        } else {
            let client = arr::Client::new(&base_url, &api_key)
                .map_err(|e| arr_call_error(source_id, e))?;
            let raw = client
                .list_episodes(series_id)
                .await
                .map_err(|e| arr_call_error(source_id, e))?;
            let trimmed: Vec<_> = raw.into_iter().map(|e| e.into_summary()).collect();
            let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
            state.arr_cache.put(source_id, &key, v).await;
            trimmed
        };

    let mut items: Vec<_> = match params.season {
        Some(s) => trimmed.into_iter().filter(|e| e.season_number == s).collect(),
        None => trimmed,
    };
    items.sort_by(|a, b| {
        a.season_number
            .cmp(&b.season_number)
            .then_with(|| a.episode_number.cmp(&b.episode_number))
    });
    Ok(Json(transcoderr_api_types::EpisodesPage { items }))
}
```

- [ ] **Step 2: Route**

In `crates/transcoderr/src/api/mod.rs`, after the `.route("/sources/:id/series/:series_id", ...)` line, append:

```rust
        .route("/sources/:id/series/:series_id/episodes", get(arr_browse::episodes))
```

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/api/arr_browse.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): GET /sources/:id/series/:series_id/episodes"
```

---

## Task 10: `POST /sources/:id/refresh`

**Files:**
- Modify: `crates/transcoderr/src/api/arr_browse.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`

Cache purge + warm. Returns 204 on success. Re-fetch is "best-effort" — we don't fail the response if the warm step times out (the next browse request will cold-fetch).

- [ ] **Step 1: Append handler**

In `crates/transcoderr/src/api/arr_browse.rs`, append at the bottom:

```rust
pub async fn refresh(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let (_row, kind, base_url, api_key) = browseable_source(&state, source_id).await?;
    state.arr_cache.invalidate(source_id).await;

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
                let trimmed: Vec<_> = raw.into_iter().map(|m| m.into_summary(&base_url)).collect();
                let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
                state.arr_cache.put(source_id, CACHE_KEY_MOVIES, v).await;
            }
        }
        arr::Kind::Sonarr => {
            if let Ok(raw) = client.list_series().await {
                let trimmed: Vec<_> = raw.into_iter().map(|s| s.into_summary(&base_url)).collect();
                let v = serde_json::to_value(&trimmed).unwrap_or(serde_json::Value::Null);
                state.arr_cache.put(source_id, CACHE_KEY_SERIES, v).await;
            }
        }
        arr::Kind::Lidarr => {} // not browseable in v1
    }
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Route**

In `crates/transcoderr/src/api/mod.rs`, append after the episodes route:

```rust
        .route("/sources/:id/refresh", post(arr_browse::refresh))
```

`post` is already imported at the top of the file; no import change needed.

- [ ] **Step 3: Build**

Run: `cargo build -p transcoderr 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/api/arr_browse.rs crates/transcoderr/src/api/mod.rs
git commit -m "feat(api): POST /sources/:id/refresh purges + warms the *arr cache"
```

---

## Task 11: `POST /sources/:id/transcode` + integration tests

**Files:**
- Modify: `crates/transcoderr/src/api/arr_browse.rs`
- Modify: `crates/transcoderr/src/api/mod.rs`
- Create: `crates/transcoderr/tests/arr_browse.rs`

The big one. Validates source, fans out across all enabled flows for the source's kind via `db::flows::list_enabled_for_kind`, synthesizes a webhook-shaped payload, inserts one pending job per flow.

- [ ] **Step 1: Append handler**

In `crates/transcoderr/src/api/arr_browse.rs`, append at the bottom:

```rust
pub async fn transcode(
    State(state): State<AppState>,
    Path(source_id): Path<i64>,
    Json(req): Json<transcoderr_api_types::TranscodeReq>,
) -> Result<Json<transcoderr_api_types::TranscodeResp>, (StatusCode, Json<ApiError>)> {
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
            Ok(run_id) => runs.push(transcoderr_api_types::TranscodeRunRef {
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

    state.arr_cache.invalidate(source_id).await;
    tracing::info!(source_id, runs = runs.len(), file_path = %req.file_path, "manual transcode enqueued");
    Ok(Json(transcoderr_api_types::TranscodeResp { runs }))
}

fn synthesize_payload(
    kind: arr::Kind,
    req: &transcoderr_api_types::TranscodeReq,
) -> serde_json::Value {
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
```

- [ ] **Step 2: Route**

In `crates/transcoderr/src/api/mod.rs`, append:

```rust
        .route("/sources/:id/transcode", post(arr_browse::transcode))
```

- [ ] **Step 3: Create the integration test file**

Create `crates/transcoderr/tests/arr_browse.rs`:

```rust
//! Integration tests for the *arr browse + transcode endpoints. Spins
//! up wiremock as a fake Radarr/Sonarr; confirms the trimmed shapes,
//! the cache, the validation gates, and the transcode fan-out.

mod common;

use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn auth_token(app: &common::TestApp) -> String {
    use transcoderr::db::api_tokens;
    let made = api_tokens::create(&app.pool, "test").await.unwrap();
    made.token
}

async fn create_auto_provisioned_source(
    app: &common::TestApp,
    arr: &MockServer,
    kind: &str,
    name: &str,
) -> i64 {
    // Mock the *arr's POST /api/v3/notification (called by source create).
    Mock::given(method("POST"))
        .and(path("/api/v3/notification"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 1, "name": "transcoderr-x",
            "implementation": "Webhook", "configContract": "WebhookSettings", "fields": []
        })))
        .mount(arr)
        .await;
    let token = auth_token(app).await;
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .bearer_auth(&token)
        .json(&json!({
            "kind": kind, "name": name,
            "config": { "base_url": arr.uri(), "api_key": "k" },
            "secret_token": ""
        }))
        .send().await.unwrap()
        .json().await.unwrap();
    resp["id"].as_i64().unwrap()
}

#[tokio::test]
async fn browse_movies_returns_trimmed_payload() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Dune", "year": 2021, "hasFile": true,
              "images": [{ "coverType": "poster", "remoteUrl": "https://image.tmdb.org/d.jpg" }],
              "movieFile": { "path": "/movies/Dune.mkv", "size": 42_000_000_000_i64,
                             "mediaInfo": { "videoCodec": "x265", "resolution": "3840x2160" },
                             "quality": { "quality": { "name": "Bluray-2160p" } } } },
            { "id": 2, "title": "Tenet",  "year": 2020, "hasFile": false, "images": [] }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 2);
    let items = r["items"].as_array().unwrap();
    // Sort default = title, so Dune before Tenet.
    assert_eq!(items[0]["title"], "Dune");
    assert_eq!(items[0]["poster_url"], "https://image.tmdb.org/d.jpg");
    assert_eq!(items[0]["has_file"], true);
    assert_eq!(items[0]["file"]["codec"], "x265");
    assert_eq!(items[0]["file"]["resolution"], "3840x2160");
    assert_eq!(items[1]["has_file"], false);
    assert!(items[1]["file"].is_null());
}

#[tokio::test]
async fn browse_movies_search_filters_server_side() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Dune",  "hasFile": false, "images": [] },
            { "id": 2, "title": "Tenet", "hasFile": false, "images": [] },
            { "id": 3, "title": "Heat",  "hasFile": false, "images": [] }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies?search=eat", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 1);
    assert_eq!(r["items"][0]["title"], "Heat");
}

#[tokio::test]
async fn browse_movies_pagination() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!(
            (1..=25).map(|i| json!({
                "id": i, "title": format!("Movie {:03}", i),
                "hasFile": false, "images": []
            })).collect::<Vec<_>>()
        )))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies?page=2&limit=10", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 25);
    assert_eq!(r["page"], 2);
    assert_eq!(r["limit"], 10);
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 10);
    assert_eq!(items[0]["title"], "Movie 011");
}

#[tokio::test]
async fn browse_series_returns_trimmed_payload() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "sonarr", "son").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/series"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Foundation", "year": 2021,
              "images": [{ "coverType": "poster", "remoteUrl": "https://art/p.jpg" }],
              "statistics": { "seasonCount": 2, "episodeCount": 20, "episodeFileCount": 18 } }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["items"][0]["title"], "Foundation");
    assert_eq!(r["items"][0]["season_count"], 2);
    assert_eq!(r["items"][0]["episode_file_count"], 18);
}

#[tokio::test]
async fn browse_episodes_filters_by_season() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "sonarr", "son").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/episode"))
        .and(query_param("seriesId", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "seasonNumber": 1, "episodeNumber": 1, "title": "Pilot",  "hasFile": false },
            { "id": 2, "seasonNumber": 2, "episodeNumber": 1, "title": "S2E1",   "hasFile": false }
        ])))
        .mount(&arr)
        .await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series/10/episodes?season=2", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "S2E1");
}

#[tokio::test]
async fn browse_rejects_non_auto_provisioned_source() {
    let app = common::boot().await;
    // Create a manual (legacy v0.9.x-shape) radarr source: empty config.
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .bearer_auth(&token)
        .json(&json!({
            "kind": "webhook", "name": "manual",
            "config": {}, "secret_token": "tok"
        }))
        .send().await.unwrap()
        .json().await.unwrap();
    let id = resp["id"].as_i64().unwrap();

    let r = client
        .get(format!("{}/api/sources/{}/movies", app.url, id))
        .bearer_auth(&token)
        .send().await.unwrap();
    assert_eq!(r.status(), 400);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["code"], "source.not_browseable");
}

#[tokio::test]
async fn browse_surfaces_arr_error() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&arr)
        .await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = client
        .get(format!("{}/api/sources/{}/movies", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap();
    assert_eq!(r.status(), 502);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["code"], "arr.upstream");
    let msg = body["message"].as_str().unwrap();
    assert!(msg.contains("401"), "got: {msg}");
}

async fn seed_radarr_flow(pool: &sqlx::SqlitePool, name: &str, enabled: bool) -> i64 {
    let yaml = format!(
        "name: {name}\ntriggers:\n  - radarr: [downloaded]\nplan:\n  steps: []\n"
    );
    let parsed = serde_json::json!({
        "name": name,
        "triggers": [{ "Radarr": ["downloaded"] }],
        "plan": { "steps": [] }
    });
    let now = transcoderr::db::now_unix();
    let enabled_int = if enabled { 1 } else { 0 };
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO flows (name, enabled, yaml_source, parsed_json, version, updated_at) \
         VALUES (?, ?, ?, ?, 1, ?) RETURNING id"
    )
    .bind(name)
    .bind(enabled_int)
    .bind(&yaml)
    .bind(parsed.to_string())
    .bind(now)
    .fetch_one(pool).await.unwrap()
}

async fn seed_sonarr_flow(pool: &sqlx::SqlitePool, name: &str) -> i64 {
    let yaml = format!(
        "name: {name}\ntriggers:\n  - sonarr: [downloaded]\nplan:\n  steps: []\n"
    );
    let parsed = serde_json::json!({
        "name": name,
        "triggers": [{ "Sonarr": ["downloaded"] }],
        "plan": { "steps": [] }
    });
    let now = transcoderr::db::now_unix();
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO flows (name, enabled, yaml_source, parsed_json, version, updated_at) \
         VALUES (?, 1, ?, ?, 1, ?) RETURNING id"
    )
    .bind(name)
    .bind(&yaml)
    .bind(parsed.to_string())
    .bind(now)
    .fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn transcode_endpoint_fans_out_across_enabled_flows() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;
    let f1 = seed_radarr_flow(&app.pool, "rad-1", true).await;
    let f2 = seed_radarr_flow(&app.pool, "rad-2", true).await;
    let _disabled = seed_radarr_flow(&app.pool, "rad-disabled", false).await;
    let _sonarr_only = seed_sonarr_flow(&app.pool, "son-only").await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .post(format!("{}/api/sources/{}/transcode", app.url, source_id))
        .bearer_auth(&token)
        .json(&json!({
            "file_path": "/movies/Dune.mkv",
            "title": "Dune",
            "movie_id": 7
        }))
        .send().await.unwrap()
        .json().await.unwrap();
    let runs = r["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "expected fan-out across 2 enabled radarr flows");
    let flow_ids: Vec<i64> = runs.iter().map(|x| x["flow_id"].as_i64().unwrap()).collect();
    assert!(flow_ids.contains(&f1));
    assert!(flow_ids.contains(&f2));
    // Disabled and sonarr-only flows did NOT enqueue jobs.
    let cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs")
        .fetch_one(&app.pool).await.unwrap();
    assert_eq!(cnt, 2);
}

#[tokio::test]
async fn transcode_returns_409_when_no_matching_flows() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;
    // No flows seeded.
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{}/api/sources/{}/transcode", app.url, source_id))
        .bearer_auth(&token)
        .json(&json!({
            "file_path": "/movies/Dune.mkv",
            "title": "Dune"
        }))
        .send().await.unwrap();
    assert_eq!(r.status(), 409);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["code"], "no_enabled_flows");
}

#[tokio::test]
async fn transcode_synthesized_payload_shape_radarr() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;
    seed_radarr_flow(&app.pool, "rad-1", true).await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let _: serde_json::Value = client
        .post(format!("{}/api/sources/{}/transcode", app.url, source_id))
        .bearer_auth(&token)
        .json(&json!({
            "file_path": "/movies/Dune.mkv",
            "title": "Dune",
            "movie_id": 7
        }))
        .send().await.unwrap()
        .json().await.unwrap();
    let payload: String = sqlx::query_scalar(
        "SELECT trigger_payload_json FROM jobs ORDER BY id DESC LIMIT 1"
    ).fetch_one(&app.pool).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(v["eventType"], "Manual");
    assert_eq!(v["movie"]["id"], 7);
    assert_eq!(v["movie"]["title"], "Dune");
    assert_eq!(v["movieFile"]["path"], "/movies/Dune.mkv");
    assert_eq!(v["_transcoderr_manual"], true);
}

#[tokio::test]
async fn transcode_synthesized_payload_shape_sonarr() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "sonarr", "son").await;
    seed_sonarr_flow(&app.pool, "son-1").await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let _: serde_json::Value = client
        .post(format!("{}/api/sources/{}/transcode", app.url, source_id))
        .bearer_auth(&token)
        .json(&json!({
            "file_path": "/tv/Foundation/S01E03.mkv",
            "title": "Foundation",
            "series_id": 1, "episode_id": 100
        }))
        .send().await.unwrap()
        .json().await.unwrap();
    let payload: String = sqlx::query_scalar(
        "SELECT trigger_payload_json FROM jobs ORDER BY id DESC LIMIT 1"
    ).fetch_one(&app.pool).await.unwrap();
    let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(v["eventType"], "Manual");
    assert_eq!(v["series"]["id"], 1);
    assert_eq!(v["series"]["title"], "Foundation");
    assert_eq!(v["episodes"][0]["id"], 100);
    assert_eq!(v["episodeFile"]["path"], "/tv/Foundation/S01E03.mkv");
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p transcoderr --test arr_browse 2>&1 | tail -20`
Expected: all 11 tests pass.

Run wider sweep: `cargo test -p transcoderr --lib --tests --locked 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: all `ok` (pre-existing metrics flake aside).

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add crates/transcoderr/src/api/arr_browse.rs crates/transcoderr/src/api/mod.rs crates/transcoderr/tests/arr_browse.rs
git commit -m "feat(api): POST /sources/:id/transcode + integration tests"
```

---

## Task 12: Frontend types + api client

**Files:**
- Create: `web/src/types-arr.ts`
- Modify: `web/src/api/client.ts`

- [ ] **Step 1: Create the types file**

Create `web/src/types-arr.ts`:

```ts
export interface FileSummary {
  path: string;
  size: number;
  codec: string | null;
  quality: string | null;
  resolution: string | null;
}

export interface MovieSummary {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  has_file: boolean;
  file: FileSummary | null;
}

export interface SeriesSummary {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  season_count: number;
  episode_count: number;
  episode_file_count: number;
}

export interface SeasonSummary {
  number: number;
  episode_count: number;
  episode_file_count: number;
  monitored: boolean;
}

export interface SeriesDetail {
  id: number;
  title: string;
  year: number | null;
  overview: string | null;
  poster_url: string | null;
  fanart_url: string | null;
  seasons: SeasonSummary[];
}

export interface EpisodeSummary {
  id: number;
  season_number: number;
  episode_number: number;
  title: string;
  air_date: string | null;
  has_file: boolean;
  file: FileSummary | null;
}

export interface MoviesPage {
  items: MovieSummary[];
  total: number;
  page: number;
  limit: number;
}

export interface SeriesPage {
  items: SeriesSummary[];
  total: number;
  page: number;
  limit: number;
}

export interface EpisodesPage {
  items: EpisodeSummary[];
}

export interface BrowseParams {
  search?: string;
  sort?: "title" | "year";
  page?: number;
  limit?: number;
}

export interface TranscodeReq {
  file_path: string;
  title: string;
  movie_id?: number;
  series_id?: number;
  episode_id?: number;
}

export interface TranscodeRunRef {
  flow_id: number;
  flow_name: string;
  run_id: number;
}

export interface TranscodeResp {
  runs: TranscodeRunRef[];
}
```

- [ ] **Step 2: Extend the api client**

In `web/src/api/client.ts`, find the `settings:` block. Just below it (still inside the `export const api = { ... }` object), insert:

```ts
  arr: {
    movies: (sourceId: number, params: import("../types-arr").BrowseParams) => {
      const q = new URLSearchParams(
        Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])
      ).toString();
      return req<import("../types-arr").MoviesPage>(`/sources/${sourceId}/movies${q ? `?${q}` : ""}`);
    },
    series: (sourceId: number, params: import("../types-arr").BrowseParams) => {
      const q = new URLSearchParams(
        Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])
      ).toString();
      return req<import("../types-arr").SeriesPage>(`/sources/${sourceId}/series${q ? `?${q}` : ""}`);
    },
    seriesGet: (sourceId: number, seriesId: number) =>
      req<import("../types-arr").SeriesDetail>(`/sources/${sourceId}/series/${seriesId}`),
    episodes: (sourceId: number, seriesId: number, season?: number) => {
      const q = season != null ? `?season=${season}` : "";
      return req<import("../types-arr").EpisodesPage>(`/sources/${sourceId}/series/${seriesId}/episodes${q}`);
    },
    transcode: (sourceId: number, body: import("../types-arr").TranscodeReq) =>
      req<import("../types-arr").TranscodeResp>(`/sources/${sourceId}/transcode`, {
        method: "POST",
        body: JSON.stringify(body),
      }),
    refresh: (sourceId: number) =>
      req<void>(`/sources/${sourceId}/refresh`, { method: "POST" }),
  },
```

- [ ] **Step 3: Build**

Run: `cd web && npm run build 2>&1 | tail -3`
Expected: clean TS build.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add web/src/types-arr.ts web/src/api/client.ts
git commit -m "feat(web): types-arr + api.arr.{movies,series,seriesGet,episodes,transcode,refresh}"
```

---

## Task 13: Shared frontend components

**Files:**
- Create: `web/src/components/source-picker.tsx`
- Create: `web/src/components/poster-grid.tsx`
- Create: `web/src/components/detail-panel.tsx`
- Create: `web/src/components/transcode-button.tsx`
- Modify: `web/src/index.css` (append styles)

Four shared components used by both Radarr and Sonarr pages.

- [ ] **Step 1: source-picker.tsx**

Create `web/src/components/source-picker.tsx`:

```tsx
import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import type { Source } from "../types";

interface Props {
  kind: "radarr" | "sonarr";
  value: number | null;
  onChange: (sourceId: number) => void;
}

const STORAGE_PREFIX = "transcoderr.last_source.";

export function lastSourceKey(kind: string): string {
  return `${STORAGE_PREFIX}${kind}`;
}

export default function SourcePicker({ kind, value, onChange }: Props) {
  const sources = useQuery({ queryKey: ["sources"], queryFn: api.sources.list });
  const matching = (sources.data ?? []).filter(
    (s: Source) =>
      s.kind === kind && s.config?.arr_notification_id != null,
  );

  // On first load (or when matching list changes), restore last-used or pick first.
  useEffect(() => {
    if (value != null) return;
    if (matching.length === 0) return;
    const remembered = localStorage.getItem(lastSourceKey(kind));
    const rememberedId = remembered ? parseInt(remembered, 10) : NaN;
    const found = matching.find((s) => s.id === rememberedId);
    onChange((found ?? matching[0]).id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matching.length]);

  if (sources.isLoading) return <div className="muted">loading sources…</div>;
  if (matching.length === 0) {
    return (
      <div className="empty">
        No auto-provisioned {kind} sources.{" "}
        <a href="/sources">Add one on the Sources page.</a>
      </div>
    );
  }

  return (
    <select
      className="source-picker"
      value={value ?? ""}
      onChange={(e) => {
        const id = parseInt(e.target.value, 10);
        localStorage.setItem(lastSourceKey(kind), String(id));
        onChange(id);
      }}
    >
      {matching.map((s) => (
        <option key={s.id} value={s.id}>
          {s.name}
        </option>
      ))}
    </select>
  );
}
```

- [ ] **Step 2: poster-grid.tsx**

Create `web/src/components/poster-grid.tsx`:

```tsx
import type { ReactNode } from "react";

interface Item {
  id: number;
  title: string;
  year: number | null;
  poster_url: string | null;
  has_file?: boolean;
}

interface Props {
  items: Item[];
  onSelect: (id: number) => void;
  selectedId: number | null;
  renderBadge?: (item: Item) => ReactNode;
}

export default function PosterGrid({ items, onSelect, selectedId, renderBadge }: Props) {
  if (items.length === 0) {
    return <div className="empty">No matches.</div>;
  }
  return (
    <div className="poster-grid">
      {items.map((it) => (
        <button
          type="button"
          key={it.id}
          className={"poster-card" + (selectedId === it.id ? " is-selected" : "")}
          onClick={() => onSelect(it.id)}
        >
          {it.poster_url ? (
            <img className="poster-img" src={it.poster_url} alt={it.title} loading="lazy" />
          ) : (
            <div className="poster-img poster-img-placeholder">🎬</div>
          )}
          <div className="poster-meta">
            <div className="poster-title">{it.title}</div>
            <div className="poster-sub">
              {it.year ?? ""}
              {renderBadge?.(it)}
            </div>
          </div>
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: transcode-button.tsx**

Create `web/src/components/transcode-button.tsx`:

```tsx
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import type { TranscodeReq, TranscodeResp } from "../types-arr";

interface Props {
  sourceId: number;
  payload: TranscodeReq;
  disabled?: boolean;
  disabledReason?: string;
}

export default function TranscodeButton({
  sourceId,
  payload,
  disabled,
  disabledReason,
}: Props) {
  const qc = useQueryClient();
  const [result, setResult] = useState<TranscodeResp | null>(null);
  const [error, setError] = useState<string | null>(null);

  const mut = useMutation({
    mutationFn: () => api.arr.transcode(sourceId, payload),
    onSuccess: (data) => {
      setResult(data);
      setError(null);
      qc.invalidateQueries({ queryKey: ["runs"] });
    },
    onError: (e: any) => {
      setError(e?.message ?? String(e));
      setResult(null);
    },
  });

  if (disabled) {
    return (
      <button type="button" className="mock-button" disabled title={disabledReason ?? ""}>
        Transcode
      </button>
    );
  }

  return (
    <div className="transcode-action">
      <button
        type="button"
        className="mock-button"
        disabled={mut.isPending}
        onClick={() => mut.mutate()}
      >
        {mut.isPending ? "Queueing…" : "Transcode"}
      </button>
      {result && (
        <div className="hint" style={{ color: "var(--ok)" }}>
          Queued {result.runs.length} run{result.runs.length === 1 ? "" : "s"}:{" "}
          {result.runs.map((r, i) => (
            <span key={r.run_id}>
              {i > 0 && ", "}
              <a href={`/runs/${r.run_id}`}>
                {r.flow_name} #{r.run_id}
              </a>
            </span>
          ))}
        </div>
      )}
      {error && (
        <div className="hint" style={{ color: "var(--bad)" }}>
          {error}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 4: detail-panel.tsx**

Create `web/src/components/detail-panel.tsx`:

```tsx
import type { ReactNode } from "react";

interface Props {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
}

export default function DetailPanel({ open, onClose, children }: Props) {
  if (!open) return null;
  return (
    <>
      <div className="detail-scrim" onClick={onClose} />
      <aside className="detail-panel">
        <button
          type="button"
          className="detail-close"
          onClick={onClose}
          aria-label="Close detail panel"
        >
          ×
        </button>
        <div className="detail-body">{children}</div>
      </aside>
    </>
  );
}

export function FileSummaryRow({
  label,
  value,
}: {
  label: string;
  value: string | null;
}) {
  if (!value) return null;
  return (
    <div className="detail-row">
      <span className="detail-label">{label}</span>
      <span className="detail-value">{value}</span>
    </div>
  );
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}
```

- [ ] **Step 5: Append styles**

In `web/src/index.css`, append at the bottom:

```css
/* ---- arr browse pages ---------------------------------------------------- */

.source-picker {
  background: var(--surface-2);
  color: var(--text);
  border: 1px solid var(--border);
  border-radius: var(--r-1);
  padding: 6px 10px;
  font-size: 12px;
}

.browse-toolbar {
  display: flex;
  gap: 12px;
  align-items: center;
  margin-bottom: var(--space-4);
  flex-wrap: wrap;
}

.poster-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  gap: 14px;
}

.poster-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-2);
  padding: 0;
  text-align: left;
  cursor: pointer;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  transition: border-color 120ms ease, background 120ms ease;
}
.poster-card:hover { border-color: var(--accent-soft); }
.poster-card.is-selected { border-color: var(--accent); }

.poster-img {
  width: 100%;
  aspect-ratio: 2 / 3;
  object-fit: cover;
  background: var(--surface-2);
}
.poster-img-placeholder {
  display: flex; align-items: center; justify-content: center;
  font-size: 32px; color: var(--text-faint);
}

.poster-meta { padding: 8px 10px; }
.poster-title { font-size: 12px; font-weight: 600; color: var(--text); }
.poster-sub   { font-size: 10px; color: var(--text-faint); margin-top: 2px; }

.detail-scrim {
  position: fixed; inset: 0; background: rgba(0, 0, 0, 0.45); z-index: 40;
}
.detail-panel {
  position: fixed; top: 0; right: 0; bottom: 0;
  width: min(420px, 90vw);
  background: var(--bg-deep);
  border-left: 1px solid var(--border);
  z-index: 41;
  overflow-y: auto;
  padding: var(--space-5);
  animation: slide-in 220ms ease;
}
@keyframes slide-in { from { transform: translateX(100%); } to { transform: translateX(0); } }
.detail-close {
  position: absolute; top: 8px; right: 8px;
  background: transparent; border: 0; color: var(--text-dim);
  font-size: 22px; cursor: pointer;
}
.detail-body { padding-top: 16px; }
.detail-row { display: flex; justify-content: space-between; padding: 4px 0; font-size: 12px; }
.detail-label { color: var(--text-faint); }
.detail-value { color: var(--text); font-family: var(--font-mono); font-size: 11px; }

.transcode-action { margin-top: 12px; display: flex; flex-direction: column; gap: 6px; }

.season-tabs {
  display: flex; gap: 4px; border-bottom: 1px solid var(--border);
  margin-bottom: var(--space-4); flex-wrap: wrap;
}
.season-tab {
  background: transparent; border: 0; padding: 8px 14px;
  color: var(--text-dim); cursor: pointer; font-size: 12px;
  border-bottom: 2px solid transparent;
}
.season-tab.is-active { color: var(--accent); border-bottom-color: var(--accent); }

.episode-row {
  display: grid;
  grid-template-columns: 60px 1fr auto;
  gap: 12px; align-items: center;
  padding: 8px 0; border-bottom: 1px solid var(--border); font-size: 12px;
}
.episode-num { color: var(--text-faint); font-family: var(--font-mono); }
.episode-title { color: var(--text); }
```

- [ ] **Step 6: Build**

Run: `cd web && npm run build 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add web/src/components/source-picker.tsx web/src/components/poster-grid.tsx web/src/components/detail-panel.tsx web/src/components/transcode-button.tsx web/src/index.css
git commit -m "feat(web): shared components — source-picker, poster-grid, detail-panel, transcode-button"
```

---

## Task 14: Radarr browse page

**Files:**
- Create: `web/src/pages/radarr.tsx`

The Radarr movies page. Source picker + search + sort + poster grid + detail panel + transcode action.

- [ ] **Step 1: Create the page**

Create `web/src/pages/radarr.tsx`:

```tsx
import { useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import SourcePicker from "../components/source-picker";
import PosterGrid from "../components/poster-grid";
import DetailPanel, { FileSummaryRow, formatBytes } from "../components/detail-panel";
import TranscodeButton from "../components/transcode-button";
import type { MovieSummary } from "../types-arr";

export default function Radarr() {
  const qc = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const setSourceId = (id: number) => {
    const sp = new URLSearchParams(searchParams);
    sp.set("source", String(id));
    setSearchParams(sp, { replace: true });
  };

  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [sort, setSort] = useState<"title" | "year">("title");
  const [page, setPage] = useState(1);
  const [selectedId, setSelectedId] = useState<number | null>(null);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(search), 250);
    return () => clearTimeout(t);
  }, [search]);

  useEffect(() => setPage(1), [debounced, sort, sourceId]);

  const movies = useQuery({
    queryKey: ["arr.movies", sourceId, debounced, sort, page],
    queryFn: () =>
      api.arr.movies(sourceId!, { search: debounced, sort, page, limit: 48 }),
    enabled: sourceId != null,
    staleTime: 5 * 60_000,
  });

  const selected = movies.data?.items.find((m) => m.id === selectedId) ?? null;

  return (
    <div className="page">
      <h1>Browse Radarr</h1>
      <div className="browse-toolbar">
        <SourcePicker kind="radarr" value={sourceId} onChange={setSourceId} />
        <input
          className="mock-input"
          placeholder="Search movies…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, maxWidth: 320 }}
        />
        <select
          className="source-picker"
          value={sort}
          onChange={(e) => setSort(e.target.value as "title" | "year")}
        >
          <option value="title">Sort: title</option>
          <option value="year">Sort: year</option>
        </select>
        <button
          type="button"
          className="mock-button"
          onClick={async () => {
            if (sourceId == null) return;
            await api.arr.refresh(sourceId);
            qc.invalidateQueries({ queryKey: ["arr.movies", sourceId] });
          }}
          disabled={sourceId == null}
        >
          Refresh
        </button>
      </div>

      {sourceId == null && (
        <div className="empty">Pick a Radarr source to browse.</div>
      )}
      {movies.isError && (
        <div className="empty">
          Couldn't load movies: {(movies.error as any)?.message ?? "unknown error"}
        </div>
      )}
      {movies.isLoading && sourceId != null && (
        <div className="empty">Loading library…</div>
      )}
      {movies.data && (
        <>
          <PosterGrid
            items={movies.data.items}
            onSelect={setSelectedId}
            selectedId={selectedId}
          />
          <Pager
            page={movies.data.page}
            limit={movies.data.limit}
            total={movies.data.total}
            onChange={setPage}
          />
        </>
      )}

      <DetailPanel open={selected != null} onClose={() => setSelectedId(null)}>
        {selected && (
          <MovieDetail
            movie={selected}
            sourceId={sourceId!}
            onTranscoded={() => setSelectedId(null)}
          />
        )}
      </DetailPanel>
    </div>
  );
}

function MovieDetail({
  movie,
  sourceId,
  onTranscoded: _onTranscoded,
}: {
  movie: MovieSummary;
  sourceId: number;
  onTranscoded: () => void;
}) {
  return (
    <>
      {movie.poster_url && (
        <img
          src={movie.poster_url}
          alt={movie.title}
          style={{ width: "100%", borderRadius: 6, marginBottom: 12 }}
        />
      )}
      <h2 style={{ margin: 0 }}>{movie.title}</h2>
      <div className="muted" style={{ marginBottom: 12 }}>
        {movie.year ?? ""}
      </div>
      {movie.file ? (
        <>
          <FileSummaryRow label="Path" value={movie.file.path} />
          <FileSummaryRow label="Size" value={formatBytes(movie.file.size)} />
          <FileSummaryRow label="Codec" value={movie.file.codec} />
          <FileSummaryRow label="Resolution" value={movie.file.resolution} />
          <FileSummaryRow label="Quality" value={movie.file.quality} />
        </>
      ) : (
        <div className="hint">No file imported yet.</div>
      )}
      <TranscodeButton
        sourceId={sourceId}
        disabled={!movie.has_file}
        disabledReason="no file imported yet"
        payload={{
          file_path: movie.file?.path ?? "",
          title: movie.title,
          movie_id: movie.id,
        }}
      />
    </>
  );
}

function Pager({
  page,
  limit,
  total,
  onChange,
}: {
  page: number;
  limit: number;
  total: number;
  onChange: (p: number) => void;
}) {
  const lastPage = Math.max(1, Math.ceil(total / limit));
  if (lastPage <= 1) return null;
  return (
    <div style={{ display: "flex", gap: 8, marginTop: 16, alignItems: "center" }}>
      <button
        type="button"
        className="mock-button"
        onClick={() => onChange(page - 1)}
        disabled={page <= 1}
      >
        ←
      </button>
      <span className="muted">
        Page {page} of {lastPage} ({total} total)
      </span>
      <button
        type="button"
        className="mock-button"
        onClick={() => onChange(page + 1)}
        disabled={page >= lastPage}
      >
        →
      </button>
    </div>
  );
}
```

- [ ] **Step 2: Build**

Run: `cd web && npm run build 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add web/src/pages/radarr.tsx
git commit -m "feat(web): pages/radarr.tsx — movies grid, search, sort, detail panel"
```

---

## Task 15: Sonarr browse page + series detail

**Files:**
- Create: `web/src/pages/sonarr.tsx`
- Create: `web/src/pages/sonarr-series.tsx`

Two pages — series grid and series-detail (seasons tabs + episode rows). Both ship together.

- [ ] **Step 1: Create the series-grid page**

Create `web/src/pages/sonarr.tsx`:

```tsx
import { useEffect, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import SourcePicker from "../components/source-picker";
import PosterGrid from "../components/poster-grid";

export default function Sonarr() {
  const qc = useQueryClient();
  const nav = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const setSourceId = (id: number) => {
    const sp = new URLSearchParams(searchParams);
    sp.set("source", String(id));
    setSearchParams(sp, { replace: true });
  };

  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [sort, setSort] = useState<"title" | "year">("title");
  const [page, setPage] = useState(1);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(search), 250);
    return () => clearTimeout(t);
  }, [search]);
  useEffect(() => setPage(1), [debounced, sort, sourceId]);

  const series = useQuery({
    queryKey: ["arr.series", sourceId, debounced, sort, page],
    queryFn: () =>
      api.arr.series(sourceId!, { search: debounced, sort, page, limit: 48 }),
    enabled: sourceId != null,
    staleTime: 5 * 60_000,
  });

  return (
    <div className="page">
      <h1>Browse Sonarr</h1>
      <div className="browse-toolbar">
        <SourcePicker kind="sonarr" value={sourceId} onChange={setSourceId} />
        <input
          className="mock-input"
          placeholder="Search series…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ flex: 1, maxWidth: 320 }}
        />
        <select
          className="source-picker"
          value={sort}
          onChange={(e) => setSort(e.target.value as "title" | "year")}
        >
          <option value="title">Sort: title</option>
          <option value="year">Sort: year</option>
        </select>
        <button
          type="button"
          className="mock-button"
          onClick={async () => {
            if (sourceId == null) return;
            await api.arr.refresh(sourceId);
            qc.invalidateQueries({ queryKey: ["arr.series", sourceId] });
          }}
          disabled={sourceId == null}
        >
          Refresh
        </button>
      </div>

      {sourceId == null && (
        <div className="empty">Pick a Sonarr source to browse.</div>
      )}
      {series.isError && (
        <div className="empty">
          Couldn't load series: {(series.error as any)?.message ?? "unknown error"}
        </div>
      )}
      {series.isLoading && sourceId != null && (
        <div className="empty">Loading library…</div>
      )}
      {series.data && (
        <>
          <PosterGrid
            items={series.data.items}
            selectedId={null}
            onSelect={(id) => nav(`/sonarr/series/${id}?source=${sourceId}`)}
          />
        </>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Create the series-detail page**

Create `web/src/pages/sonarr-series.tsx`:

```tsx
import { useEffect, useState } from "react";
import { useParams, useSearchParams, Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import TranscodeButton from "../components/transcode-button";
import { formatBytes } from "../components/detail-panel";
import type { EpisodeSummary } from "../types-arr";

export default function SonarrSeries() {
  const { seriesId } = useParams();
  const [searchParams] = useSearchParams();
  const sourceId = searchParams.get("source")
    ? parseInt(searchParams.get("source")!, 10)
    : null;
  const seriesIdNum = seriesId ? parseInt(seriesId, 10) : null;

  const detail = useQuery({
    queryKey: ["arr.series.get", sourceId, seriesIdNum],
    queryFn: () => api.arr.seriesGet(sourceId!, seriesIdNum!),
    enabled: sourceId != null && seriesIdNum != null,
    staleTime: 5 * 60_000,
  });

  const [activeSeason, setActiveSeason] = useState<number | null>(null);
  useEffect(() => {
    if (activeSeason != null) return;
    const seasons = detail.data?.seasons ?? [];
    const firstReal = seasons.find((s) => s.number > 0) ?? seasons[0];
    if (firstReal) setActiveSeason(firstReal.number);
  }, [detail.data, activeSeason]);

  const episodes = useQuery({
    queryKey: ["arr.episodes", sourceId, seriesIdNum, activeSeason],
    queryFn: () =>
      api.arr.episodes(sourceId!, seriesIdNum!, activeSeason ?? undefined),
    enabled: sourceId != null && seriesIdNum != null && activeSeason != null,
    staleTime: 5 * 60_000,
  });

  if (sourceId == null) {
    return (
      <div className="page">
        <Link to="/sonarr">← Back to series list</Link>
        <div className="empty">No source selected.</div>
      </div>
    );
  }

  return (
    <div className="page">
      <Link to={`/sonarr?source=${sourceId}`}>← Back to series list</Link>
      {detail.isLoading && <div className="empty">Loading…</div>}
      {detail.data && (
        <>
          <div style={{ display: "flex", gap: 16, marginTop: 12, marginBottom: 16 }}>
            {detail.data.poster_url && (
              <img
                src={detail.data.poster_url}
                alt={detail.data.title}
                style={{ width: 140, borderRadius: 6 }}
              />
            )}
            <div>
              <h1 style={{ margin: 0 }}>{detail.data.title}</h1>
              <div className="muted">{detail.data.year ?? ""}</div>
              {detail.data.overview && (
                <p style={{ marginTop: 8, fontSize: 13, color: "var(--text-dim)" }}>
                  {detail.data.overview}
                </p>
              )}
            </div>
          </div>

          <div className="season-tabs">
            {detail.data.seasons.map((s) => (
              <button
                key={s.number}
                type="button"
                className={
                  "season-tab" + (activeSeason === s.number ? " is-active" : "")
                }
                onClick={() => setActiveSeason(s.number)}
              >
                {s.number === 0 ? "Specials" : `Season ${s.number}`}
                <span className="muted" style={{ marginLeft: 6, fontSize: 10 }}>
                  {s.episode_file_count}/{s.episode_count}
                </span>
              </button>
            ))}
          </div>

          {episodes.isLoading && <div className="empty">Loading episodes…</div>}
          {episodes.data && (
            <div>
              {episodes.data.items.map((ep) => (
                <EpisodeRow
                  key={ep.id}
                  episode={ep}
                  sourceId={sourceId}
                  seriesId={seriesIdNum!}
                  seriesTitle={detail.data!.title}
                />
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

function EpisodeRow({
  episode,
  sourceId,
  seriesId,
  seriesTitle,
}: {
  episode: EpisodeSummary;
  sourceId: number;
  seriesId: number;
  seriesTitle: string;
}) {
  return (
    <div className="episode-row">
      <span className="episode-num">
        {String(episode.season_number).padStart(2, "0")}×
        {String(episode.episode_number).padStart(2, "0")}
      </span>
      <div>
        <div className="episode-title">{episode.title}</div>
        {episode.file && (
          <div className="muted" style={{ fontSize: 10, fontFamily: "var(--font-mono)" }}>
            {episode.file.codec ?? ""} · {episode.file.resolution ?? ""} ·{" "}
            {formatBytes(episode.file.size)}
          </div>
        )}
        {!episode.has_file && (
          <div className="hint">no file imported</div>
        )}
      </div>
      <TranscodeButton
        sourceId={sourceId}
        disabled={!episode.has_file}
        disabledReason="no file imported yet"
        payload={{
          file_path: episode.file?.path ?? "",
          title: seriesTitle,
          series_id: seriesId,
          episode_id: episode.id,
        }}
      />
    </div>
  );
}
```

- [ ] **Step 3: Build**

Run: `cd web && npm run build 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add web/src/pages/sonarr.tsx web/src/pages/sonarr-series.tsx
git commit -m "feat(web): pages/sonarr.tsx + sonarr-series.tsx (seasons tabs, episode rows)"
```

---

## Task 16: Wire routes + sidebar nav-links

**Files:**
- Modify: `web/src/App.tsx`
- Modify: `web/src/components/sidebar.tsx`

- [ ] **Step 1: Add the routes**

In `web/src/App.tsx`, find the `<Routes>` block. Add three routes near the existing `/sources` route (alphabetic-ish, doesn't really matter):

```tsx
          <Route path="/radarr" element={<Radarr />} />
          <Route path="/sonarr" element={<Sonarr />} />
          <Route path="/sonarr/series/:seriesId" element={<SonarrSeries />} />
```

At the top of the file, add the imports next to the other page imports:

```tsx
import Radarr from "./pages/radarr";
import Sonarr from "./pages/sonarr";
import SonarrSeries from "./pages/sonarr-series";
```

- [ ] **Step 2: Add the sidebar nav-links**

In `web/src/components/sidebar.tsx`, find the `links` array (the "Operate" section). Modify it to:

```tsx
const links: [string, string][] = [
  ["/dashboard", "Dashboard"],
  ["/flows", "Flows"],
  ["/runs", "Runs"],
  ["/radarr", "Browse Radarr"],
  ["/sonarr", "Browse Sonarr"],
];
```

- [ ] **Step 3: Build**

Run: `cd web && npm run build 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git branch --show-current   # must print: feature/arr-browse
git add web/src/App.tsx web/src/components/sidebar.tsx
git commit -m "feat(web): routes + sidebar links for /radarr and /sonarr"
```

---

## Task 17: Verification

**Files:** none (verification only)

- [ ] **Step 1: Confirm branch + clean state**

Run: `git branch --show-current && git status --short`
Expected: `feature/arr-browse`, no uncommitted changes.

- [ ] **Step 2: Run the per-crate test suite**

Run: `cargo test -p transcoderr --locked --lib --tests 2>&1 | grep -E "^test result|FAILED" | head -30`
Expected: all `ok`. Pre-existing `metrics_endpoint_responds_with_text_format` flake may surface — known.

- [ ] **Step 3: Confirm new tests are present**

Run: `cargo test -p transcoderr --lib arr:: 2>&1 | grep -E "^test " | head -20`
Expected: ~16 arr-namespace tests (cache: 3, browse: 6, mod: original 8 + 4 new GET-method tests).

Run: `cargo test -p transcoderr --test arr_browse 2>&1 | grep -E "^test " | head -15`
Expected: 11 integration tests, all `... ok`.

- [ ] **Step 4: Frontend build sanity**

Run: `cd web && npm run build 2>&1 | tail -5`
Expected: clean TypeScript build.

- [ ] **Step 5: Branch commit list**

Run: `git log --oneline main..HEAD`
Expected (in order, plus the spec commit `23266a5` already on the branch):

```
feat(web): routes + sidebar links for /radarr and /sonarr
feat(web): pages/sonarr.tsx + sonarr-series.tsx (seasons tabs, episode rows)
feat(web): pages/radarr.tsx — movies grid, search, sort, detail panel
feat(web): shared components — source-picker, poster-grid, detail-panel, transcode-button
feat(web): types-arr + api.arr.{movies,series,seriesGet,episodes,transcode,refresh}
feat(api): POST /sources/:id/transcode + integration tests
feat(api): POST /sources/:id/refresh purges + warms the *arr cache
feat(api): GET /sources/:id/series/:series_id/episodes
feat(api): GET /sources/:id/series and /series/:id
feat(api): GET /sources/:id/movies proxy with cache+search+sort+paginate
feat(http): AppState gains arr_cache; built at serve()
feat(db): list_enabled_for_kind for any-event manual fan-out
feat(arr): Client::list_movies/list_series/get_series/list_episodes
feat(arr): browse types + trim impls (movies/series/episodes)
feat(arr): TTL cache for browse-proxy responses
feat(api-types): browse + transcode response shapes
docs(spec): radarr/sonarr browse-and-transcode pages
```

- [ ] **Step 6: Manual smoke test (optional but recommended)**

Run: `cargo run -p transcoderr -- serve --config <your-cfg>` and:
1. Navigate to /radarr — confirm the Source picker shows your auto-provisioned radarr.
2. Confirm the poster grid loads (cold-fetch may take 1-3s with a spinner).
3. Type in the search box — confirm grid filters with ~250ms debounce.
4. Switch sort to "year" — confirm reorder.
5. Click a poster → detail panel slides in with file metadata.
6. Click Transcode → confirm "Queued N runs" hint with run links → click a link → /runs shows the new run.
7. Click Refresh → confirm cold-fetch happens again.
8. Navigate to /sonarr → repeat (1)-(4) for series.
9. Click a series → /sonarr/series/:id → confirm seasons tabs work + episode rows render with per-row Transcode buttons.
10. Click an episode's Transcode → confirm enqueue.
11. Test the "no file imported" path: find a movie/episode where Radarr/Sonarr knows the title but hasn't grabbed a file yet; confirm the Transcode button is disabled with the expected tooltip.

- [ ] **Step 7: (No commit — verification only.)** Branch is ready for review/merge.
