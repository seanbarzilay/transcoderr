mod common;
use common::boot;

#[tokio::test]
async fn metrics_endpoint_responds_with_text_format() {
    let app = boot().await;
    let body = reqwest::get(format!("{}/metrics", app.url)).await.unwrap().text().await.unwrap();
    assert!(body.contains("# HELP transcoderr_queue_depth"),
        "expected HELP line for transcoderr_queue_depth in:\n{}", body);
    assert!(body.contains("transcoderr_queue_depth 0"),
        "expected gauge value emitted at boot in:\n{}", body);
}
