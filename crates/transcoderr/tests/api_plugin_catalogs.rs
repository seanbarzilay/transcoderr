mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn plugin_catalogs_crud() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // The migration seeds one official catalog. List should show it.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list1.len(), 1);
    assert_eq!(list1[0]["name"], "transcoderr official");

    // Create a private catalog with an auth header.
    let resp: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "internal",
            "url": "https://internal.example/index.json",
            "auth_header": "Bearer xyz",
            "priority": 5,
        }))
        .send().await.unwrap().json().await.unwrap();
    let id = resp["id"].as_i64().unwrap();

    // List now has two.
    let list2: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(list2.len(), 2);

    // Delete returns 204.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 204);

    // Deleting again returns 404.
    let resp = client
        .delete(format!("{}/api/plugin-catalogs/{id}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn browse_returns_entries_and_errors_per_catalog() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = boot().await;
    let client = reqwest::Client::new();

    // Replace the seed catalog with a wiremock-backed one so the test
    // doesn't try to fetch from the real internet.
    let list1: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list1[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();

    let server_ok = MockServer::start().await;
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "size-report",
                "version": "0.1.0",
                "summary": "size",
                "tarball_url": "https://example.com/x.tgz",
                "tarball_sha256": "h",
                "kind": "subprocess",
                "provides_steps": ["size.report.before"]
            }]
        })))
        .mount(&server_ok).await;

    client.post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({
            "name": "ok",
            "url": format!("{}/index.json", server_ok.uri()),
        }))
        .send().await.unwrap();

    let body: serde_json::Value = client
        .get(format!("{}/api/plugin-catalog-entries", app.url))
        .send().await.unwrap().json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["name"], "size-report");
    assert_eq!(entries[0]["catalog_name"], "ok");
    assert!(body["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn install_then_uninstall_round_trip() {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let app = boot().await;
    let client = reqwest::Client::new();

    // Build a tarball for a one-step plugin "demo" providing "demo.do".
    let manifest = "name = \"demo\"\nversion = \"0.1.0\"\nkind = \"subprocess\"\nentrypoint = \"bin/run\"\nprovides_steps = [\"demo.do\"]\n";
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/").unwrap(); hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
        tar.append(&hdr, std::io::empty()).unwrap();
        let body = manifest.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/manifest.toml").unwrap();
        hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();
        let run = b"#!/bin/sh\nread A\nread B\necho '{\"event\":\"result\",\"status\":\"ok\",\"outputs\":{}}'\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("demo/bin/run").unwrap();
        hdr.set_mode(0o755); hdr.set_size(run.len() as u64); hdr.set_cksum();
        tar.append(&hdr, &run[..]).unwrap();
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let mut h = Sha256::new(); h.update(&bytes);
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();

    // Mock catalog hosting the tarball.
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "demo",
                "version": "0.1.0",
                "summary": "demo",
                "tarball_url": format!("{url}/demo.tar.gz"),
                "tarball_sha256": sha,
                "kind": "subprocess",
                "provides_steps": ["demo.do"]
            }]
        })))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/demo.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server).await;

    // Replace the seed catalog with this mock.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();
    let create: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({"name": "mock", "url": format!("{url}/index.json")}))
        .send().await.unwrap().json().await.unwrap();
    let cid = create["id"].as_i64().unwrap();

    // Install via the API.
    let resp = client
        .post(format!("{}/api/plugin-catalog-entries/{cid}/demo/install", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // /api/plugins now lists demo with provides_steps from the manifest.
    let plugins: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "demo");
    let pid = plugins[0]["id"].as_i64().unwrap();

    // Uninstall via the API.
    let resp = client
        .delete(format!("{}/api/plugins/{pid}", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 204);

    let plugins_after: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send().await.unwrap().json().await.unwrap();
    assert!(plugins_after.is_empty());
}
