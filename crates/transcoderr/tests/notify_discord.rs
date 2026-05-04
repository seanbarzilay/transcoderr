use serde_json::json;
use transcoderr::notifiers;

#[tokio::test]
async fn discord_posts_to_url() {
    // Spin a tiny mock server that captures the body.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let received = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let recv = received.clone();
    tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = tokio::io::AsyncReadExt::read(&mut s, &mut buf)
            .await
            .unwrap();
        *recv.lock().await = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tokio::io::AsyncWriteExt::write_all(
            &mut s,
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
    });

    let n = notifiers::build("discord", &json!({"url": format!("http://{addr}/x")})).unwrap();
    n.send("hello", &json!({})).await.unwrap();
    let body = received.lock().await.clone();
    assert!(body.contains("\"content\":\"hello\""));
}
