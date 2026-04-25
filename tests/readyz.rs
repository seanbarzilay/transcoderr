mod common;
use common::boot;

#[tokio::test]
async fn readyz_returns_200_after_boot() {
    let app = boot().await;
    let r = reqwest::get(format!("{}/readyz", app.url)).await.unwrap();
    assert_eq!(r.status(), 200);
    let h = reqwest::get(format!("{}/healthz", app.url)).await.unwrap();
    assert_eq!(h.status(), 200);
}
