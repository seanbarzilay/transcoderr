//! Integration tests for `POST /api/flows/validate`. The validator is
//! the static counterpart to the runtime `if:` evaluator — which
//! silently swallows compile/exec errors and treats them as `false`.
//! This endpoint is what closes the "guard typo silently disables a
//! branch" failure mode.

mod common;

use serde_json::json;
use serial_test::serial;

async fn auth_token(app: &common::TestApp) -> String {
    use transcoderr::db::api_tokens;
    let made = api_tokens::create(&app.pool, "test").await.unwrap();
    made.token
}

async fn validate(
    client: &reqwest::Client,
    app: &common::TestApp,
    token: &str,
    yaml: &str,
) -> serde_json::Value {
    client
        .post(format!("{}/api/flows/validate", app.url))
        .bearer_auth(token)
        .json(&json!({ "yaml": yaml }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn validate_clean_flow_returns_ok_with_no_issues() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = validate(
        &client,
        &app,
        &token,
        "name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - id: a\n    if: probe.streams != null\n    then:\n      - return: ok\n",
    )
    .await;
    assert_eq!(r["ok"], true, "got {r}");
    assert!(r["issues"].as_array().unwrap().is_empty());
}

#[tokio::test]
#[serial]
async fn validate_surfaces_cel_compile_error_in_if() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = validate(
        &client,
        &app,
        &token,
        "name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - id: a\n    if: \"this is not valid cel ((\"\n    then:\n      - return: x\n",
    )
    .await;
    assert_eq!(r["ok"], false);
    let issues = r["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["kind"], "condition_compile_error");
    assert_eq!(issues[0]["path"], "steps[0].if");
}

#[tokio::test]
#[serial]
async fn validate_surfaces_template_compile_error_in_with() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = validate(
        &client,
        &app,
        &token,
        "name: t\nenabled: true\ntriggers:\n  - radarr: [downloaded]\nsteps:\n  - id: n\n    use: notify\n    with:\n      template: \"hello {{ this is not valid (( }}\"\n",
    )
    .await;
    assert_eq!(r["ok"], false);
    let issues = r["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["kind"], "template_compile_error");
    assert_eq!(issues[0]["path"], "steps[0].with.template");
}

#[tokio::test]
#[serial]
async fn validate_yaml_parse_error_short_circuits() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r = validate(&client, &app, &token, "not: valid: yaml: at all:").await;
    assert_eq!(r["ok"], false);
    let issues = r["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0]["kind"], "yaml_parse_error");
}
