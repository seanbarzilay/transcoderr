use serde_json::json;
use transcoderr::notifiers;

#[tokio::test]
async fn telegram_posts_to_send_message() {
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
            b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"ok\":true,\"x\":1}",
        )
        .await;
    });

    let n = notifiers::build(
        "telegram",
        &json!({
            "base_url": format!("http://{addr}"),
            "bot_token": "TEST_TOKEN",
            "chat_id": "12345",
        }),
    )
    .unwrap();
    n.send("hello from transcoderr", &json!({})).await.unwrap();

    let body = received.lock().await.clone();
    assert!(
        body.contains("/botTEST_TOKEN/sendMessage"),
        "wrong path: {body}"
    );
    assert!(
        body.contains("\"chat_id\":\"12345\""),
        "missing chat_id: {body}"
    );
    assert!(
        body.contains("\"text\":\"hello from transcoderr\""),
        "missing text: {body}"
    );
}
