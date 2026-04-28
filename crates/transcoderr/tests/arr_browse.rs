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
            // hasFile=false → filtered out by the server (browse pages
            // exist to find downloaded files, not the entire watchlist).
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
    assert_eq!(r["total"], 1, "hasFile=false items must be filtered out");
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Dune");
    assert_eq!(items[0]["poster_url"], "https://image.tmdb.org/d.jpg");
    assert_eq!(items[0]["has_file"], true);
    assert_eq!(items[0]["file"]["codec"], "x265");
    assert_eq!(items[0]["file"]["resolution"], "3840x2160");
}

#[tokio::test]
async fn browse_movies_search_filters_server_side() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Dune",  "hasFile": true, "images": [],
              "movieFile": { "path": "/m/Dune.mkv", "size": 1 } },
            { "id": 2, "title": "Tenet", "hasFile": true, "images": [],
              "movieFile": { "path": "/m/Tenet.mkv", "size": 1 } },
            { "id": 3, "title": "Heat",  "hasFile": true, "images": [],
              "movieFile": { "path": "/m/Heat.mkv", "size": 1 } }
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
                "hasFile": true, "images": [],
                "movieFile": { "path": format!("/m/{i}.mkv"), "size": 1 }
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
            { "id": 1, "seasonNumber": 1, "episodeNumber": 1, "title": "Pilot", "hasFile": true,
              "episodeFile": { "path": "/tv/s01e01.mkv", "size": 1 } },
            { "id": 2, "seasonNumber": 2, "episodeNumber": 1, "title": "S2E1",  "hasFile": true,
              "episodeFile": { "path": "/tv/s02e01.mkv", "size": 1 } }
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
async fn browse_movies_codec_and_resolution_filters() {
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "A 4K HEVC", "hasFile": true, "images": [],
              "movieFile": { "path": "/m/a.mkv", "size": 1,
                             "mediaInfo": { "videoCodec": "x265", "resolution": "3840x2160" } } },
            { "id": 2, "title": "B 1080p H264", "hasFile": true, "images": [],
              "movieFile": { "path": "/m/b.mkv", "size": 1,
                             "mediaInfo": { "videoCodec": "h264", "resolution": "1920x1080" } } },
            { "id": 3, "title": "C 4K H264", "hasFile": true, "images": [],
              "movieFile": { "path": "/m/c.mkv", "size": 1,
                             "mediaInfo": { "videoCodec": "h264", "resolution": "3840x2160" } } }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();

    // No filter: all 3 returned, available_* sets are union of values.
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 3);
    let codecs: Vec<&str> = r["available_codecs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(codecs, vec!["h264", "x265"]); // sorted, distinct
    let resolutions: Vec<&str> = r["available_resolutions"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(resolutions, vec!["1920x1080", "3840x2160"]);

    // codec=h264 narrows to 2 movies.
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies?codec=h264", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 2);
    // available_* still reflects the WHOLE library, not the filtered view.
    assert_eq!(r["available_codecs"].as_array().unwrap().len(), 2);

    // codec=h264 + resolution=3840x2160 → just movie C.
    let r: serde_json::Value = client
        .get(format!(
            "{}/api/sources/{}/movies?codec=h264&resolution=3840x2160",
            app.url, source_id
        ))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 1);
    assert_eq!(r["items"][0]["title"], "C 4K H264");
}

#[tokio::test]
async fn browse_series_aggregates_codecs_and_filters() {
    // The series list page fans out per-series episode fetches so it
    // can show codec/resolution badges and filter on them. This test
    // mocks two series, gives them different episode codecs/resolutions,
    // and verifies (a) the SeriesPage available_* sets union the values
    // across series, (b) ?codec=hevc narrows to the series whose
    // episodes have that codec.
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let source_id = create_auto_provisioned_source(&app, &arr, "sonarr", "son").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/series"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Alpha",
              "statistics": { "seasonCount": 1, "episodeCount": 2, "episodeFileCount": 2 } },
            { "id": 2, "title": "Bravo",
              "statistics": { "seasonCount": 1, "episodeCount": 1, "episodeFileCount": 1 } }
        ])))
        .mount(&arr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/episode"))
        .and(query_param("seriesId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 11, "seasonNumber": 1, "episodeNumber": 1, "title": "A1", "hasFile": true,
              "episodeFile": { "path": "/tv/a1.mkv", "size": 1,
                               "mediaInfo": { "videoCodec": "hevc", "resolution": "3840x2160" } } },
            { "id": 12, "seasonNumber": 1, "episodeNumber": 2, "title": "A2", "hasFile": true,
              "episodeFile": { "path": "/tv/a2.mkv", "size": 1,
                               "mediaInfo": { "videoCodec": "h264", "resolution": "1920x1080" } } }
        ])))
        .mount(&arr)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v3/episode"))
        .and(query_param("seriesId", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 21, "seasonNumber": 1, "episodeNumber": 1, "title": "B1", "hasFile": true,
              "episodeFile": { "path": "/tv/b1.mkv", "size": 1,
                               "mediaInfo": { "videoCodec": "h264", "resolution": "1920x1080" } } }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();

    // No filter: both series, available_* unions per-series sets.
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 2, "got {r}");
    let codecs: Vec<&str> = r["available_codecs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(codecs, vec!["h264", "hevc"]);
    let resolutions: Vec<&str> = r["available_resolutions"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(resolutions, vec!["1920x1080", "3840x2160"]);

    // Each series carries its own per-series codec set.
    let alpha = r["items"].as_array().unwrap().iter()
        .find(|s| s["title"] == "Alpha").unwrap();
    let alpha_codecs: Vec<&str> = alpha["codecs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(alpha_codecs, vec!["h264", "hevc"]);
    let bravo = r["items"].as_array().unwrap().iter()
        .find(|s| s["title"] == "Bravo").unwrap();
    let bravo_codecs: Vec<&str> = bravo["codecs"].as_array().unwrap()
        .iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(bravo_codecs, vec!["h264"]);

    // codec=hevc → only Alpha (which has at least one hevc episode).
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series?codec=hevc", app.url, source_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 1);
    assert_eq!(r["items"][0]["title"], "Alpha");
    // available_* still reflects the WHOLE library, not the filtered view.
    assert_eq!(r["available_codecs"].as_array().unwrap().len(), 2);

    // The fan-out also warmed `episodes:{id}` cache so the per-series
    // episode endpoint serves without another round-trip. Verify by
    // counting recorded *arr requests: only one /api/v3/episode call
    // per series for the whole sequence above.
    let ep1_calls = arr.received_requests().await.unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/v3/episode"
                 && r.url.query().unwrap_or("").contains("seriesId=1"))
        .count();
    assert_eq!(ep1_calls, 1, "warm episodes cache should make a single call per series");
}

#[tokio::test]
async fn browse_filters_out_undownloaded_items() {
    // Movies, series, and episodes the *arr knows about but hasn't
    // imported a file for must NOT appear in the browse results — the
    // pages exist to find files to transcode.
    let arr = MockServer::start().await;
    let app = common::boot().await;
    let radarr_id = create_auto_provisioned_source(&app, &arr, "radarr", "rad").await;

    Mock::given(method("GET"))
        .and(path("/api/v3/movie"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Imported", "hasFile": true, "images": [],
              "movieFile": { "path": "/m/i.mkv", "size": 1 } },
            { "id": 2, "title": "Wishlisted", "hasFile": false, "images": [] }
        ])))
        .mount(&arr)
        .await;

    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/movies", app.url, radarr_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    assert_eq!(r["total"], 1);
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Imported");

    let sonarr = MockServer::start().await;
    let sonarr_id = create_auto_provisioned_source(&app, &sonarr, "sonarr", "son").await;
    Mock::given(method("GET"))
        .and(path("/api/v3/series"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "title": "Has files", "year": 2020,
              "statistics": { "seasonCount": 1, "episodeCount": 10, "episodeFileCount": 7 } },
            // Series the operator added but Sonarr never imported episodes for.
            { "id": 2, "title": "Brand new", "year": 2024,
              "statistics": { "seasonCount": 1, "episodeCount": 10, "episodeFileCount": 0 } }
        ])))
        .mount(&sonarr)
        .await;
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series", app.url, sonarr_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Has files");

    Mock::given(method("GET"))
        .and(path("/api/v3/episode"))
        .and(query_param("seriesId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": 1, "seasonNumber": 1, "episodeNumber": 1, "title": "Aired", "hasFile": true,
              "episodeFile": { "path": "/tv/s01e01.mkv", "size": 1 } },
            { "id": 2, "seasonNumber": 1, "episodeNumber": 2, "title": "Future", "hasFile": false }
        ])))
        .mount(&sonarr)
        .await;
    let r: serde_json::Value = client
        .get(format!("{}/api/sources/{}/series/1/episodes", app.url, sonarr_id))
        .bearer_auth(&token)
        .send().await.unwrap()
        .json().await.unwrap();
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["title"], "Aired");
}

#[tokio::test]
async fn browse_rejects_non_auto_provisioned_source() {
    let app = common::boot().await;
    // Create a manual (legacy v0.9.x-shape) webhook-kind source: empty config.
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
        "name: {name}\ntriggers:\n  - radarr: [downloaded]\nsteps: []\n"
    );
    // Trigger serializes as a single-key map with lowercase key (see
    // crates/transcoderr/src/flow/model.rs Trigger::serialize), and Flow
    // has a top-level `steps` field — no `plan` wrapper.
    let parsed = serde_json::json!({
        "name": name,
        "triggers": [{ "radarr": ["downloaded"] }],
        "steps": []
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
        "name: {name}\ntriggers:\n  - sonarr: [downloaded]\nsteps: []\n"
    );
    let parsed = serde_json::json!({
        "name": name,
        "triggers": [{ "sonarr": ["downloaded"] }],
        "steps": []
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
