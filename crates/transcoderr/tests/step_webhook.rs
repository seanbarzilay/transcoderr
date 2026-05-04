//! Integration tests for the `webhook` builtin step. Drives the step's
//! `execute` directly against a wiremock server; bypasses the flow
//! engine because the templating + HTTP path is what we actually want
//! to verify.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use transcoderr::flow::Context;
use transcoderr::steps::webhook::WebhookStep;
use transcoderr::steps::{Step, StepProgress};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn with_map(v: Value) -> BTreeMap<String, Value> {
    match v {
        Value::Object(m) => m.into_iter().collect(),
        _ => panic!("test bug: pass an object"),
    }
}

async fn run(with: Value, ctx: &mut Context) -> anyhow::Result<()> {
    let step = WebhookStep;
    let mut cb = |_: StepProgress| {};
    step.execute(&with_map(with), ctx, &mut cb).await
}

#[tokio::test]
async fn success_2xx_step_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/notify"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(json!({"url": format!("{}/notify", server.uri())}), &mut ctx)
        .await
        .unwrap();
}

#[tokio::test]
async fn non_2xx_step_fails_with_truncated_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/notify"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal explosion"))
        .mount(&server)
        .await;

    let mut ctx = Context::for_file("/m/x.mkv");
    let err = run(json!({"url": format!("{}/notify", server.uri())}), &mut ctx)
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("500"), "must include status: {msg}");
    assert!(
        msg.contains("internal explosion"),
        "must include body: {msg}"
    );
}

#[tokio::test]
async fn non_2xx_with_ignore_errors_step_ok() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/notify"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(
        json!({
            "url": format!("{}/notify", server.uri()),
            "ignore_errors": true,
        }),
        &mut ctx,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn network_error_step_fails() {
    // Port 1 isn't listening; reqwest will get a connection refused.
    let mut ctx = Context::for_file("/m/x.mkv");
    let err = run(json!({"url": "http://127.0.0.1:1/x"}), &mut ctx)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("webhook:"), "got: {}", err);
}

#[tokio::test]
async fn network_error_with_ignore_errors_ok() {
    let mut ctx = Context::for_file("/m/x.mkv");
    run(
        json!({
            "url": "http://127.0.0.1:1/x",
            "ignore_errors": true,
        }),
        &mut ctx,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn templated_url_headers_body_round_trip() {
    let server = MockServer::start().await;
    // Wiremock asserts: must POST /notify, with X-Source header set to
    // "transcoderr", and body containing the templated file.path.
    Mock::given(method("POST"))
        .and(path("/notify"))
        .and(header("X-Source", "transcoderr"))
        .and(body_string_contains("/movies/Foo.mkv"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = Context::for_file("/movies/Foo.mkv");
    run(
        json!({
            "url": format!("{}/notify", server.uri()),
            "headers": {"X-Source": "transcoderr"},
            "body": "{{ file.path }}",
        }),
        &mut ctx,
    )
    .await
    .unwrap();
    // Mock's `expect(1)` is verified at drop.
}

#[tokio::test]
async fn body_omitted_for_get_when_unset() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let mut ctx = Context::for_file("/m/x.mkv");
    run(
        json!({
            "url": format!("{}/ping", server.uri()),
            "method": "GET",
        }),
        &mut ctx,
    )
    .await
    .unwrap();
}
