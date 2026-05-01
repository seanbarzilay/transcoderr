//! End-to-end: mock a catalog hosting a tarball that mirrors
//! `docs/plugins/size-report/`, install via the API, then run a flow
//! that exercises both step names. Asserts ctx.steps.size_report is
//! populated by the run.

mod common;

use common::boot;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Write;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn build_size_report_tarball() -> (Vec<u8>, String) {
    let manifest = "name = \"size-report\"\n\
                    version = \"0.1.0\"\n\
                    kind = \"subprocess\"\n\
                    entrypoint = \"bin/run\"\n\
                    provides_steps = [\"size.report.before\", \"size.report.after\"]\n";
    let run_script = std::fs::read_to_string("../../docs/plugins/size-report/bin/run")
        .expect("docs/plugins/size-report/bin/run readable from crate dir");

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/").unwrap();
        hdr.set_mode(0o755); hdr.set_size(0); hdr.set_cksum();
        tar.append(&hdr, std::io::empty()).unwrap();

        let body = manifest.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/manifest.toml").unwrap();
        hdr.set_mode(0o644); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();

        let body = run_script.as_bytes();
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("size-report/bin/run").unwrap();
        hdr.set_mode(0o755); hdr.set_size(body.len() as u64); hdr.set_cksum();
        tar.append(&hdr, body).unwrap();
        tar.finish().unwrap();
    }
    let bytes = gz.finish().unwrap();
    let mut h = Sha256::new(); h.update(&bytes);
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    (bytes, sha)
}

#[tokio::test]
async fn install_size_report_then_run_uses_steps() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // Mock catalog hosting size-report.
    let (bytes, sha) = build_size_report_tarball();
    let server = MockServer::start().await;
    let url = server.uri();
    Mock::given(method("GET")).and(path("/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "schema_version": 1,
            "plugins": [{
                "name": "size-report",
                "version": "0.1.0",
                "summary": "size report",
                "tarball_url": format!("{url}/sr.tar.gz"),
                "tarball_sha256": sha,
                "kind": "subprocess",
                "provides_steps": ["size.report.before", "size.report.after"]
            }]
        })))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/sr.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server).await;

    // Replace the seed catalog with the mock.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugin-catalogs", app.url))
        .send().await.unwrap().json().await.unwrap();
    let seed_id = list[0]["id"].as_i64().unwrap();
    client.delete(format!("{}/api/plugin-catalogs/{seed_id}", app.url))
        .send().await.unwrap();
    let resp: serde_json::Value = client
        .post(format!("{}/api/plugin-catalogs", app.url))
        .json(&json!({"name": "mock", "url": format!("{url}/index.json")}))
        .send().await.unwrap().json().await.unwrap();
    let cid = resp["id"].as_i64().unwrap();

    // Install.
    let resp = client
        .post(format!("{}/api/plugin-catalog-entries/{cid}/size-report/install", app.url))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);

    // Run size.report.before / .after by hand against a temp file.
    use std::collections::BTreeMap;
    use transcoderr::flow::Context;
    use transcoderr::steps::{registry, StepProgress};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Movie.mkv");
    std::fs::File::create(&path).unwrap().write_all(&vec![0u8; 1000]).unwrap();
    let mut ctx = Context::for_file(path.to_string_lossy().to_string());

    let before_step = registry::resolve("size.report.before").await
        .expect("size.report.before in registry post-install");
    before_step.execute(&BTreeMap::new(), &mut ctx, &mut |_: StepProgress| {})
        .await.unwrap();

    // Simulate a transcode that shrunk the file to 600 bytes.
    std::fs::File::create(&path).unwrap().write_all(&vec![0u8; 600]).unwrap();

    let after_step = registry::resolve("size.report.after").await.unwrap();
    after_step.execute(&BTreeMap::new(), &mut ctx, &mut |_: StepProgress| {})
        .await.unwrap();

    let report = ctx.steps.get("size_report").expect("size_report key written");
    assert_eq!(report["before_bytes"], 1000);
    assert_eq!(report["after_bytes"], 600);
    assert_eq!(report["saved_bytes"], 400);
    assert!(
        (report["ratio_pct"].as_f64().unwrap() - 40.0).abs() < 0.01,
        "ratio_pct = {:?}", report["ratio_pct"]
    );
}
