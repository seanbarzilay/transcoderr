# Radarr / Sonarr Browse-and-Transcode Pages

**Goal:** Two new pages in the transcoderr WebUI that let an operator browse the Radarr / Sonarr library, pick a movie or episode, and manually enqueue a transcode — bypassing the usual webhook-driven trigger. Operator-on-demand transcoding without leaving transcoderr.

**Branch context:** Builds on the v0.10.x auto-provision flow. Sources of `kind ∈ {radarr, sonarr, lidarr}` that are auto-provisioned (have `arr_notification_id` AND `base_url` AND `api_key` in their `config_json`) become "browseable" — manually-configured legacy v0.9.x sources do not, since they have no stored credentials.

## Non-goals

- Lidarr browse page (not in scope; same code shape would extend to it later if needed)
- Path remapping between *arr and transcoderr containers — we preserve the existing assumption that file paths from the *arr work verbatim in transcoderr's filesystem
- Browsing libraries from sources that aren't auto-provisioned
- Editing *arr-side state from transcoderr (search/import/refresh series, monitor toggles)
- Image-byte proxying — browser fetches poster URLs directly from TMDB/TheTVDB CDNs

---

## 1. User flow

Two top-level pages added under "Operate" in the sidebar:

- **`/radarr`** — Radarr browse: paginated grid of movie posters.
- **`/sonarr`** — Sonarr browse: paginated grid of series posters.
- **`/sonarr/series/:seriesId`** — series detail: seasons as tabs, episodes as rows under the active tab.

Top of every browse page: source-picker dropdown (auto-provisioned sources of the matching kind) + search input + sort dropdown.

Source-id is stored as a query param `?source=:id`. Last-used source is remembered in `localStorage` so revisiting the page lands on the same source. Switching kinds (Radarr → Sonarr via the sidebar) keeps the kind's own remembered source.

Click a poster → side detail panel slides in: full metadata, file details (path, size, codec, resolution), **Transcode** button.

Click Transcode → fan out across all enabled flows for that source's kind. Each flow gets its own pending job. Toast confirms with run count and links to `/runs`.

For Sonarr, clicking a series card pushes `/sonarr/series/:id`. The series-detail subroute renders seasons as tabs (Season 1, Season 2, …); the active tab shows episode rows. Each episode row has its own Transcode button (per-row, since episodes don't have unique posters and the detail panel adds little).

### Empty states

| Condition | Empty state |
|---|---|
| No auto-provisioned sources of the kind exist | "Add a Radarr/Sonarr source on the Sources page to browse its library" with link to /sources |
| Source picker has a value, but the *arr is unreachable | "Couldn't reach `<name>` — check `base_url`" + retry button |
| Source returns 401 | "Authentication failed — check the source's api_key" + link to /sources |
| Library is empty | "No movies/series in this Radarr/Sonarr instance yet" |

---

## 2. Backend API

All endpoints behind `require_auth` (cookie or Bearer token), under the existing `/api` prefix.

```
GET  /api/sources/:id/movies?search=&sort=&page=&limit=
  → { items: MovieSummary[], total, page, limit }

GET  /api/sources/:id/series?search=&sort=&page=&limit=
  → { items: SeriesSummary[], total, page, limit }

GET  /api/sources/:id/series/:series_id
  → SeriesDetail

GET  /api/sources/:id/series/:series_id/episodes?season=
  → { items: EpisodeSummary[] }

POST /api/sources/:id/transcode
  body: { file_path: String, title: String,
          movie_id?: i64, series_id?: i64, episode_id?: i64 }
  → { runs: [{ flow_id, flow_name, run_id }] }

POST /api/sources/:id/refresh        // purges this source's cache + warms it
  → 204
```

### Validation (shared by all proxy handlers)

1. Source row exists.
2. `Kind::parse(&row.kind).is_some()` — radarr/sonarr/lidarr only.
3. `config.arr_notification_id` is present (i.e. auto-provisioned).
4. `config.base_url` is a non-empty string.
5. `config.api_key` is a non-empty string.

If any check fails: `400 BAD_REQUEST` with the existing `ApiError { code, message }` shape — `code: "source.not_browseable"`.

If the *arr call itself fails: `502 BAD_GATEWAY` and `tracing::error!` with the response body (same pattern as the auto-provision branch in `api/sources::create`).

### Trimmed response shapes

```rust
pub struct MovieSummary {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,    // remoteUrl from images[coverType=poster], or null
    pub has_file: bool,
    pub file: Option<FileSummary>,     // None when has_file = false
}

pub struct SeriesSummary {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub poster_url: Option<String>,
    pub season_count: i32,
    pub episode_count: i32,
    pub episode_file_count: i32,        // how many of the episodes have a file
}

pub struct SeriesDetail {
    pub id: i64,
    pub title: String,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster_url: Option<String>,
    pub fanart_url: Option<String>,
    pub seasons: Vec<SeasonSummary>,
}

pub struct SeasonSummary {
    pub number: i32,
    pub episode_count: i32,
    pub episode_file_count: i32,
    pub monitored: bool,
}

pub struct EpisodeSummary {
    pub id: i64,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: String,
    pub air_date: Option<String>,       // ISO 8601 or null
    pub has_file: bool,
    pub file: Option<FileSummary>,
}

pub struct FileSummary {
    pub path: String,
    pub size: i64,                       // bytes
    pub codec: Option<String>,           // mediaInfo.videoCodec
    pub quality: Option<String>,         // quality.quality.name (e.g. "Bluray-2160p")
    pub resolution: Option<String>,      // mediaInfo.resolution (e.g. "3840x2160")
}
```

Trimming lives in per-resource `From<RadarrMovie> for MovieSummary` impls in `crates/transcoderr/src/arr/browse.rs` (separate file from `arr/mod.rs` so it doesn't pollute the existing notification client). *arr-schema knowledge stays isolated to this file.

### `POST /transcode` semantics

1. Validate the source (same as proxy handlers).
2. Synthesize a trigger payload mimicking the kind's webhook body so existing flow plan-builders that read fields like `series.title` keep working:

   For radarr:
   ```json
   { "eventType": "Manual",
     "movie": { "id": <movie_id>, "title": <title> },
     "movieFile": { "path": <file_path> },
     "_transcoderr_manual": true }
   ```

   For sonarr:
   ```json
   { "eventType": "Manual",
     "series": { "id": <series_id>, "title": <title> },
     "episodes": [{ "id": <episode_id> }],
     "episodeFile": { "path": <file_path> },
     "_transcoderr_manual": true }
   ```
3. Look up enabled flows via a new helper `db::flows::list_enabled_for_kind(pool, kind)` — returns ALL enabled flows whose YAML wires any event for that source kind, ignoring event semantics. (Manual = any-event fan-out.)
4. If 0 matching flows: `409 CONFLICT` with `ApiError { code: "no_enabled_flows", message: "..." }`.
5. For each matching flow, insert a pending job via the existing `db::jobs::insert_with_source`. Return `{ runs: [{ flow_id, flow_name, run_id }, …] }`.
6. Best-effort cache invalidation for this source-id (so any future "transcoded files" badge stays fresh).

The handler does NOT pre-flight-check that the file exists on disk — the worker will fail it the same way a webhook-triggered run would, keeping behavior consistent.

---

## 3. *arr REST endpoints consumed

| Our endpoint | *arr endpoint | Auth header |
|---|---|---|
| `GET /movies` | `GET {base}/api/v3/movie` | `X-Api-Key` |
| `GET /series` | `GET {base}/api/v3/series` | `X-Api-Key` |
| `GET /series/:id` | `GET {base}/api/v3/series/:id` | `X-Api-Key` |
| `GET /series/:id/episodes?season=` | `GET {base}/api/v3/episode?seriesId=:id` | `X-Api-Key` |

For posters/fanart: each *arr response's `images[]` array includes a `remoteUrl` field pointing to TMDB (movies) or TheTVDB (series). We surface `remoteUrl` to the browser. Browsers load images directly from `image.tmdb.org` / `artworks.thetvdb.com`, no LAN dependency on the *arr. Fall back to `/MediaCover/.../poster.jpg` (resolved against `base_url`) only if `remoteUrl` is missing.

The existing `arr::Client` is reused for the GET methods. New methods on it:
```rust
impl Client {
    pub async fn list_movies(&self) -> Result<Vec<RadarrMovie>>;
    pub async fn list_series(&self) -> Result<Vec<SonarrSeries>>;
    pub async fn get_series(&self, id: i64) -> Result<SonarrSeries>;
    pub async fn list_episodes(&self, series_id: i64) -> Result<Vec<SonarrEpisode>>;
}
```
Each follows the existing pattern: build URL, GET with `X-Api-Key`, map non-2xx to `anyhow::bail!("*arr returned {status}: {text}")`, parse JSON.

The full `RadarrMovie` / `SonarrSeries` / `SonarrEpisode` structs are defined in `crates/transcoderr/src/arr/browse.rs` with `#[serde(default)]` on optional fields and the trimming `From` impls. `#[serde(other)]` or `#[serde(flatten)]` extras are not collected — we don't need round-trip fidelity here.

---

## 4. Caching

In-memory TTL cache stored on `AppState` as `Arc<ArrCache>`. TTL = 5 minutes.

```rust
// crates/transcoderr/src/arr/cache.rs
pub struct ArrCache {
    inner: tokio::sync::RwLock<HashMap<(i64, String), CacheEntry>>,
}

struct CacheEntry {
    data: serde_json::Value,
    expires_at: std::time::Instant,
}

impl ArrCache {
    pub async fn get(&self, source_id: i64, key: &str) -> Option<serde_json::Value>;
    pub async fn put(&self, source_id: i64, key: &str, data: serde_json::Value);
    pub async fn invalidate(&self, source_id: i64);   // drops all keys for the source
}
```

Cache keys per resource: `"movies"`, `"series"`, `"series:{id}"`, `"episodes:{series_id}"`.

The cache holds the **full library** (whole *arr response, post-trimming). Search / sort / pagination happen at the proxy handler over the cached `Vec<...>` before serialization. This means every request after a cache hit is sub-millisecond.

### Invalidation

- **TTL expiry**: passive, on next read.
- **`POST /transcode`**: invalidates the affected source-id (cheap; positions us for a future "transcoded recently" badge).
- **`POST /api/sources/:id/refresh`**: explicit purge + immediate re-fetch from the *arr. The detail panel exposes this as a "Refresh from Radarr" button paired with a "last synced N min ago" timestamp.

Cold cache fetch from a big *arr can be 1-3s. The frontend shows a spinner during that period.

No upper bound on cache entry count — a homelab library trims to maybe 50 MB of JSON; bounding adds complexity for no real benefit at this scale.

---

## 5. Frontend structure

### Routes

```
/radarr                          → pages/radarr.tsx
/sonarr                          → pages/sonarr.tsx
/sonarr/series/:seriesId         → pages/sonarr-series.tsx
```

`?source=<id>` query param scopes the view to a specific source. Last-used per kind is persisted in `localStorage["transcoderr.last_source.radarr"]` etc.

### New files

```
web/src/pages/radarr.tsx                       — movies grid + search + sort + detail panel
web/src/pages/sonarr.tsx                       — series grid + search + sort
web/src/pages/sonarr-series.tsx                — series detail (seasons tabs + episode rows)
web/src/components/poster-grid.tsx             — shared poster-card grid + paginator
web/src/components/source-picker.tsx           — kind-filtered dropdown of auto-prov sources
web/src/components/detail-panel.tsx            — slide-in side panel (used by movies + series)
web/src/components/transcode-button.tsx        — calls POST /transcode, toasts on success
web/src/types-arr.ts                           — TS types matching the Rust trimmed shapes
```

### State

react-query for proxy endpoints with `staleTime: 5 * 60_000` (matches server TTL — combined client+server caching). Query keys per `(source_id, search, sort, page)`. Detail-panel data is its own query keyed on the selected item id.

Transcode action: `useMutation` posting to `/api/sources/:id/transcode`. On success: toast with run links + invalidate `runs` query so a fresh visit to `/runs` shows the new jobs at top. On 4xx/5xx: error toast with the server's message.

### `api/client.ts` additions

```ts
arr: {
  movies:    (sourceId: number, params: BrowseParams) => req<MoviesPage>(...),
  series:    (sourceId: number, params: BrowseParams) => req<SeriesPage>(...),
  seriesGet: (sourceId: number, seriesId: number)     => req<SeriesDetail>(...),
  episodes:  (sourceId: number, seriesId: number, season?: number) => req<EpisodesPage>(...),
  transcode: (sourceId: number, body: TranscodeReq)   => req<TranscodeResp>(..., { method: "POST", body }),
  refresh:   (sourceId: number)                       => req<void>(..., { method: "POST" }),
}
```

### Sidebar

Add two nav-links under "Operate":
- `Browse Radarr` → `/radarr`
- `Browse Sonarr` → `/sonarr`

Hidden on mobile? No — they're first-class pages. The mobile drawer (added in v0.10.4) handles them like any other route.

---

## 6. Error handling

Surfaced to the operator via toasts and empty states (no console-only errors).

| Scenario | Backend response | Frontend treatment |
|---|---|---|
| Source not auto-provisioned | 400 `{ code: "source.not_browseable" }` | Empty state: "This source isn't auto-provisioned. Auto-provisioning lets transcoderr browse the library." |
| *arr unreachable (timeout / connect refused) | 502 with `tracing::error!` chain | Empty state: "Couldn't reach `<source name>`. Check base_url." + Retry button |
| *arr 401 (bad api_key) | 502, body includes "401 Unauthorized" | Empty state: "Authentication failed — check the api_key in Sources" + link |
| *arr 500 / other 5xx | 502, body includes the *arr's response | Toast: "Radarr returned an error: \<excerpt\>" |
| `POST /transcode` with no enabled flows | 409 `{ code: "no_enabled_flows" }` | Toast: "No enabled flows match this source" + link to /flows |
| `POST /transcode` server error | 500 | Toast: "Failed to enqueue transcode" + log line on the server |
| File not yet imported (`has_file: false`) | n/a (frontend-only) | Transcode button disabled with tooltip "no file imported yet" |

Logs follow the existing pattern: `tracing::error!(source_id = id, error = ?e, "...")` for *arr failures; `tracing::info!` on successful transcode-enqueue with run count.

---

## 7. Testing

### Backend (`crates/transcoderr/tests/arr_browse.rs`)

- **`browse_movies_returns_trimmed_payload`** — wiremock fake Radarr returns a curated full-schema `/api/v3/movie` response; assert the trimmed `MovieSummary[]` shape (correct titles, poster_url from images[].remoteUrl, has_file, FileSummary populated).
- **`browse_movies_search_filters_server_side`** — assert `?search=Dune` returns a subset.
- **`browse_movies_pagination`** — assert `?page=2&limit=10` returns the right window plus `total`.
- **`browse_series_returns_trimmed_payload`** — same shape check for sonarr series.
- **`browse_episodes_filters_by_season`** — `?season=2` returns only s02 episodes.
- **`browse_rejects_non_auto_provisioned_source`** — POST with a legacy radarr source returns 400 `source.not_browseable`.
- **`browse_surfaces_arr_error`** — wiremock returns 401, our handler returns 502 with the *arr body in the error chain.
- **`transcode_endpoint_fans_out_across_enabled_flows`** — seed 2 enabled radarr flows + 1 disabled flow + 1 sonarr-only flow; POST /transcode for a radarr source; assert exactly 2 jobs land in `jobs` table with the synthesized payload.
- **`transcode_returns_409_when_no_matching_flows`** — no matching flows → 409 `no_enabled_flows`.
- **`transcode_synthesized_payload_shape`** — assert the inserted job's `trigger_payload_json` has the right shape (`eventType: "Manual"`, `movie/series/episode` keys present per kind, `_transcoderr_manual: true`).

### Cache (`crates/transcoderr/src/arr/cache.rs`)

- Pure-Rust unit tests; mock the clock with a `now_fn: fn() -> Instant` injection so TTL-expiry is testable without sleep.
- **`cache_returns_value_within_ttl`**
- **`cache_returns_none_after_ttl_expiry`**
- **`invalidate_drops_all_keys_for_source_id`**

### Frontend

No automated tests (consistent with the existing pages). Manual smoke checklist captured in the implementation plan:
- Source picker swaps
- Search filters with debounce
- Sort dropdown re-sorts
- Pagination scrolls + loads next page
- Click poster opens detail panel
- Transcode produces toast with run links
- /runs shows the new runs at top
- Sonarr drill-down: click series → /sonarr/series/:id → seasons tabs work → per-episode transcode buttons fire

---

## 8. Open questions / future work

(Out of scope for v1; flagged so reviewers know they're intentional gaps.)

- **"Already queued" detection** — when the operator clicks Transcode, we don't check whether the same `(source_id, file_path)` already has a pending/running job. Could cause double-encoding on rapid double-click. Mitigation: frontend disables the button while the mutation is in flight. Server-side dedup is a separate ticket.
- **Per-source default flow** — current design fans out to all matching flows. If the operator has multiple flows and wants the manual-trigger to use only one, they'd need to disable the others. A `default_flow_id` field on sources would address this; deferred.
- **Lidarr browse** — the source kind is supported in auto-provisioning but not in this browse design. Adding it is the same shape (proxy + trim + fan-out) but with `/api/v3/artist|album|track`.
- **DB-persisted cache** — option C from the brainstorm. If 5-min TTL turns out to be too coarse or too aggressive, revisit.

---

## 9. File layout

```
crates/transcoderr/src/arr/browse.rs            [create: per-resource From impls + RadarrMovie/SonarrSeries/SonarrEpisode types]
crates/transcoderr/src/arr/cache.rs             [create: ArrCache + tests]
crates/transcoderr/src/arr/mod.rs               [modify: list_movies/list_series/get_series/list_episodes on Client; pub mod browse, cache]
crates/transcoderr/src/api/arr_browse.rs        [create: proxy handlers + transcode endpoint]
crates/transcoderr/src/api/mod.rs               [modify: route the new endpoints]
crates/transcoderr/src/db/flows.rs              [modify: add list_enabled_for_kind helper]
crates/transcoderr/src/http/mod.rs              [modify: AppState gains arr_cache: Arc<ArrCache>]
crates/transcoderr/src/main.rs                  [modify: build the cache, inject into AppState]
crates/transcoderr-api-types/src/lib.rs         [modify: MovieSummary / SeriesSummary / SeriesDetail / EpisodeSummary / FileSummary / TranscodeReq / TranscodeResp]
crates/transcoderr/tests/arr_browse.rs          [create: wiremock-backed integration tests]

web/src/pages/radarr.tsx                        [create]
web/src/pages/sonarr.tsx                        [create]
web/src/pages/sonarr-series.tsx                 [create]
web/src/components/poster-grid.tsx              [create]
web/src/components/source-picker.tsx            [create]
web/src/components/detail-panel.tsx             [create]
web/src/components/transcode-button.tsx         [create]
web/src/types-arr.ts                            [create]
web/src/api/client.ts                           [modify: arr: { movies, series, seriesGet, episodes, transcode, refresh }]
web/src/components/sidebar.tsx                  [modify: two new nav-links under "Operate"]
web/src/App.tsx                                 [modify: three new routes]
```
