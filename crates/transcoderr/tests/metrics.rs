mod common;
use common::boot;

#[tokio::test]
async fn metrics_endpoint_responds_with_text_format() {
    let app = boot().await;
    let body = reqwest::get(format!("{}/metrics", app.url)).await.unwrap().text().await.unwrap();
    assert!(body.contains("transcoderr_queue_depth") || body.contains("# HELP"),
        "expected prometheus format in:\n{}", body);
}
