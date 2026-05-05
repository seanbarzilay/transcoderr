//! Integration test for `GET /api/step-kinds`. The endpoint is what
//! lets MCP-driven flow authoring discover the built-in catalog and
//! plugin step shapes without grepping source.

mod common;

use serial_test::serial;

async fn auth_token(app: &common::TestApp) -> String {
    use transcoderr::db::api_tokens;
    let made = api_tokens::create(&app.pool, "test").await.unwrap();
    made.token
}

#[tokio::test]
#[serial]
async fn step_kinds_returns_built_in_catalog_with_metadata() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/step-kinds", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let arr = r.as_array().expect("response is an array");
    assert!(!arr.is_empty(), "step-kinds should not be empty after boot");

    // Sanity-check a few well-known built-ins are present with the
    // expected shape.
    let by_name: std::collections::HashMap<&str, &serde_json::Value> = arr
        .iter()
        .map(|v| (v["name"].as_str().unwrap(), v))
        .collect();

    let probe = by_name.get("probe").expect("probe should be registered");
    assert_eq!(probe["kind"], "builtin");
    assert_eq!(probe["executor"], "coordinator_only");

    // `transcode` is a remote-eligible built-in; executor should be "any".
    let transcode = by_name
        .get("transcode")
        .expect("transcode should be registered");
    assert_eq!(transcode["kind"], "builtin");
    assert_eq!(transcode["executor"], "any");

    // The output is sorted by name — adjacent compare.
    let names: Vec<&str> = arr.iter().map(|v| v["name"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "step-kinds response should be sorted by name"
    );
}

#[tokio::test]
#[serial]
async fn step_kinds_includes_with_schemas_for_known_built_ins() {
    let app = common::boot().await;
    let token = auth_token(&app).await;
    let client = reqwest::Client::new();
    let r: serde_json::Value = client
        .get(format!("{}/api/step-kinds", app.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let by_name: std::collections::HashMap<&str, &serde_json::Value> = r
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["name"].as_str().unwrap(), v))
        .collect();

    // Tier-1 step with a real config: `output` should expose a schema
    // declaring its `mode` field.
    let output = by_name.get("output").unwrap();
    let schema = &output["with_schema"];
    assert!(
        !schema.is_null(),
        "output should have a non-null with_schema"
    );
    let props = &schema["properties"];
    assert!(
        props.get("mode").is_some(),
        "output schema should describe a `mode` field; got {schema}"
    );

    // No-config step: `probe` exposes the empty schema (an explicit
    // signal that it accepts no `with:` keys).
    let probe = by_name.get("probe").unwrap();
    let probe_schema = &probe["with_schema"];
    assert_eq!(probe_schema["type"], "object", "probe schema is empty obj");
    assert_eq!(
        probe_schema["additionalProperties"], false,
        "probe schema should reject any `with:` key"
    );
}
