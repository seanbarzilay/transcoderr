mod common;

use common::boot;
use serde_json::json;

#[tokio::test]
async fn runs_list_empty() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let runs: Vec<serde_json::Value> = client
        .get(format!("{}/api/runs", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(runs.is_empty());
}

#[tokio::test]
async fn sources_crud() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // empty list
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/sources", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());

    // create
    let resp: serde_json::Value = client
        .post(format!("{}/api/sources", app.url))
        .json(&json!({
            "kind": "webhook",
            "name": "my-webhook",
            "config": {"foo": "bar"},
            "secret_token": "tok123"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = resp["id"].as_i64().unwrap();

    // get
    let detail: serde_json::Value = client
        .get(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["name"].as_str().unwrap(), "my-webhook");
    assert_eq!(detail["kind"].as_str().unwrap(), "webhook");

    // list has one
    let list2: Vec<serde_json::Value> = client
        .get(format!("{}/api/sources", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list2.len(), 1);

    // update
    let upd = client
        .put(format!("{}/api/sources/{id}", app.url))
        .json(&json!({
            "name": "my-webhook-renamed",
            "config": {"foo": "baz"},
            "secret_token": "tok456"
        }))
        .send()
        .await
        .unwrap();
    assert!(upd.status().is_success());

    let detail2: serde_json::Value = client
        .get(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail2["name"].as_str().unwrap(), "my-webhook-renamed");

    // test-fire stub returns 204
    let tf = client
        .post(format!("{}/api/sources/{id}/test-fire", app.url))
        .send()
        .await
        .unwrap();
    assert_eq!(tf.status(), 204);

    // delete
    let del = client
        .delete(format!("{}/api/sources/{id}", app.url))
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());

    let list3: Vec<serde_json::Value> = client
        .get(format!("{}/api/sources", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list3.is_empty());
}

#[tokio::test]
async fn plugins_list_empty() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let plugins: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // No plugins seeded in test DB; list should be empty
    assert!(plugins.is_empty());
}

/// Drop a plugin directory on disk, call sync_discovered, and assert that
/// both the list and detail endpoints surface the manifest and README.
/// This is the contract the new Plugins page depends on.
#[tokio::test]
async fn plugins_detail_returns_manifest_and_readme() {
    use std::fs;

    let app = boot().await;
    let client = reqwest::Client::new();

    // Lay down a fake plugin in {data_dir}/plugins/example/.
    let plugin_dir = app.data_dir.join("plugins").join("example");
    fs::create_dir_all(plugin_dir.join("bin")).unwrap();
    fs::write(
        plugin_dir.join("manifest.toml"),
        r#"
name = "example"
version = "0.2.0"
kind = "subprocess"
entrypoint = "bin/run"
provides_steps = ["example.do", "example.undo"]
capabilities = ["fs.read"]
"#,
    )
    .unwrap();
    fs::write(
        plugin_dir.join("README.md"),
        "# Example\n\nUse it like `use: example.do`.\n",
    )
    .unwrap();
    fs::write(plugin_dir.join("schema.json"), r#"{"type":"object"}"#).unwrap();

    // Same call boot() would make at startup if there had been plugins.
    let discovered = transcoderr::plugins::discover(&app.data_dir.join("plugins")).unwrap();
    transcoderr::db::plugins::sync_discovered(
        &app.pool,
        &discovered,
        &std::collections::HashMap::new(),
    )
    .await
    .unwrap();

    // List surfaces provides_steps and *not* an enabled flag.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/plugins", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    let row = &list[0];
    assert_eq!(row["name"].as_str().unwrap(), "example");
    assert_eq!(row["version"].as_str().unwrap(), "0.2.0");
    assert_eq!(row["kind"].as_str().unwrap(), "subprocess");
    assert_eq!(
        row["provides_steps"].as_array().unwrap(),
        &vec![json!("example.do"), json!("example.undo")]
    );
    assert!(
        row.get("enabled").is_none(),
        "enabled should not appear in the list response"
    );

    // Detail returns the manifest *and* the README contents.
    let id = row["id"].as_i64().unwrap();
    let detail: serde_json::Value = client
        .get(format!("{}/api/plugins/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["name"].as_str().unwrap(), "example");
    assert_eq!(
        detail["capabilities"].as_array().unwrap(),
        &vec![json!("fs.read")]
    );
    assert!(detail["readme"].as_str().unwrap().contains("# Example"));
    assert!(detail["path"]
        .as_str()
        .unwrap()
        .ends_with("plugins/example"));
}

/// PATCH /api/plugins/:id should be gone -- the toggle was removed in
/// favour of "all plugins are always-on".
#[tokio::test]
async fn plugins_patch_endpoint_no_longer_exists() {
    let app = boot().await;
    let client = reqwest::Client::new();
    let resp = client
        .patch(format!("{}/api/plugins/1", app.url))
        .json(&json!({"enabled": false}))
        .send()
        .await
        .unwrap();
    // axum returns 405 Method Not Allowed when the path exists for a
    // different method (GET).
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn notifiers_crud() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // empty list
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/api/notifiers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());

    // create
    let resp: serde_json::Value = client
        .post(format!("{}/api/notifiers", app.url))
        .json(&json!({
            "name": "test-notifier",
            "kind": "webhook",
            "config": {"url": "https://example.com/hook"}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = resp["id"].as_i64().unwrap();

    // get
    let detail: serde_json::Value = client
        .get(format!("{}/api/notifiers/{id}", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["name"].as_str().unwrap(), "test-notifier");
    assert_eq!(detail["kind"].as_str().unwrap(), "webhook");

    // list has one
    let list2: Vec<serde_json::Value> = client
        .get(format!("{}/api/notifiers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list2.len(), 1);

    // delete
    let del = client
        .delete(format!("{}/api/notifiers/{id}", app.url))
        .send()
        .await
        .unwrap();
    assert!(del.status().is_success());

    let list3: Vec<serde_json::Value> = client
        .get(format!("{}/api/notifiers", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list3.is_empty());
}

#[tokio::test]
async fn settings_get_and_patch() {
    let app = boot().await;
    let client = reqwest::Client::new();

    // get all
    let settings: std::collections::HashMap<String, serde_json::Value> = client
        .get(format!("{}/api/settings", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // auth.enabled is seeded as 'false'
    assert!(settings.contains_key("auth.enabled"));

    // patch the run-concurrency setting
    let patch = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"runs.max_concurrent": "4"}))
        .send()
        .await
        .unwrap();
    assert!(patch.status().is_success());

    let settings2: std::collections::HashMap<String, serde_json::Value> = client
        .get(format!("{}/api/settings", app.url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings2["runs.max_concurrent"].as_str().unwrap(), "4");
}

/// Reproduces the Enclave 2026-05-09 critical: anyone with worker-token
/// access (which is unauthenticated to obtain via POST /worker/enroll)
/// could PATCH /api/settings with `{"auth.password_hash": "<known>"}`
/// and then log in with the matching plaintext.
///
/// Pre-fix: this test FAILS at the `attacker login MUST fail` assertion
/// because the password hash gets overwritten and `/api/auth/login`
/// happily verifies against the attacker's hash.
///
/// Post-fix: PATCH silently drops every `auth.*` key; the legitimate
/// hash stays, attacker login returns 401, operator login still works.
#[tokio::test]
async fn settings_patch_blocks_auth_password_hash_overwrite() {
    let app = boot().await;
    let client = reqwest::ClientBuilder::new()
        .cookie_store(true)
        .build()
        .unwrap();

    // Operator enables auth and sets a legitimate password via the
    // first-run path. The handler hashes it server-side.
    let r = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({
            "auth.enabled": "true",
            "auth.password": "operator-password",
        }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "first-run enable failed: {}",
        r.status()
    );

    // Sanity: operator can log in.
    let login = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "operator-password"}))
        .send()
        .await
        .unwrap();
    assert!(
        login.status().is_success(),
        "operator login should succeed: {}",
        login.status()
    );

    // Attacker move: forge an Argon2 hash for a chosen password and
    // try to overwrite the stored hash via PATCH /api/settings. The
    // request rides on the just-issued session cookie (the cookie jar
    // captured it from /auth/login), simulating an attacker who has
    // any authenticated identity — including a worker token from the
    // unauthenticated /worker/enroll endpoint.
    let attacker_password = "attacker-password";
    let attacker_hash =
        transcoderr::api::auth::hash_password(attacker_password).expect("argon2 hash builds");
    let r = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.password_hash": attacker_hash}))
        .send()
        .await
        .unwrap();
    // The PATCH itself is allowed to return 204 (silently dropped).
    // The vulnerability isn't the response — it's whether the stored
    // hash actually changed. The login attempt below is the real assertion.
    assert!(
        r.status().is_success() || r.status().as_u16() == 400,
        "unexpected PATCH status: {}",
        r.status()
    );

    // The actual security assertion: attacker's chosen plaintext must
    // NOT log in. If this returns 204, `auth.password_hash` was
    // successfully overwritten and the takeover chain works.
    let attack_login = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": attacker_password}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        attack_login.status().as_u16(),
        401,
        "ATTACK SUCCEEDED: PATCH /api/settings with auth.password_hash \
         overwrote the operator hash; attacker logged in as admin. This \
         is the Enclave 2026-05-09 critical."
    );

    // Belt-and-braces: legitimate password still works after the
    // attack attempt.
    let login = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "operator-password"}))
        .send()
        .await
        .unwrap();
    assert!(
        login.status().is_success(),
        "operator login should still work after attempted overwrite: {}",
        login.status()
    );
}

/// Companion to the `auth.password_hash` overwrite test: the report's
/// "alternative" attack of disabling auth wholesale via PATCH must be
/// blocked the same way.
#[tokio::test]
async fn settings_patch_blocks_auth_enabled_disable() {
    let app = boot().await;
    let client = reqwest::ClientBuilder::new()
        .cookie_store(true)
        .build()
        .unwrap();

    // Enable auth.
    let r = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({
            "auth.enabled": "true",
            "auth.password": "operator-password",
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    // Log in to obtain a session cookie so the next PATCH is "authed".
    let login = client
        .post(format!("{}/api/auth/login", app.url))
        .json(&json!({"password": "operator-password"}))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success());

    // Try to disable auth via PATCH. Pre-fix this would write
    // `auth.enabled = "false"` and any subsequent unauthenticated
    // request would walk straight through `require_auth`.
    let r = client
        .patch(format!("{}/api/settings", app.url))
        .json(&json!({"auth.enabled": "false"}))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success() || r.status().as_u16() == 400,
        "unexpected PATCH status: {}",
        r.status()
    );

    // Verify auth.enabled stayed "true". Use a fresh client (no cookie)
    // and read /api/settings — that endpoint is on the protected router,
    // so an UNAUTHENTICATED read should now 401, proving auth is still
    // on.
    let bare = reqwest::Client::new();
    let resp = bare
        .get(format!("{}/api/settings", app.url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status().as_u16(),
        401,
        "PATCH /api/settings disabled auth — anyone can now read settings unauthenticated"
    );
}

#[tokio::test]
async fn dry_run_basic() {
    let app = boot().await;
    let client = reqwest::Client::new();

    let yaml = r#"
name: test
triggers:
  - radarr: [downloaded]
steps:
  - use: probe
  - if: "true"
    then:
      - use: remux
    else:
      - use: transcode
"#;

    let probe = json!({
        "streams": [{"codec_type": "video", "codec_name": "h264"}]
    });

    let resp: serde_json::Value = client
        .post(format!("{}/api/dry-run", app.url))
        .json(&json!({
            "yaml": yaml,
            "file_path": "/fake/file.mkv",
            "probe": probe
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let steps = resp["steps"].as_array().unwrap();
    // probe step, if-true conditional, remux step
    assert!(!steps.is_empty());
    assert_eq!(steps[0]["kind"].as_str().unwrap(), "step");
    assert_eq!(steps[0]["use_or_label"].as_str().unwrap(), "probe");

    // The probe we sent should come back in the response
    assert_eq!(
        resp["probe"]["streams"][0]["codec_name"].as_str().unwrap(),
        "h264"
    );
}
