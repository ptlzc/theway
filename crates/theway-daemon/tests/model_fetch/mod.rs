//! Local-fixture tests for [`fetch_models`](crate::model_fetch::fetch_models):
//! a one-shot TCP server serves a canned `/models` response so the HTTP path
//! (URL join, bearer auth, error mapping) is exercised without a real provider.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

use crate::model_fetch::fetch_models;

/// Serve exactly one HTTP request and return its captured head (request line +
/// headers) plus the port the caller should use as `base_url`.
async fn serve_once(body: &'static str, status: &'static str) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = vec![0u8; 8192];
        let read = socket.read(&mut buffer).await.unwrap();
        let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        let _ = socket.shutdown().await;
        let _ = tx.send(request);
    });
    (format!("http://{addr}/v1"), rx)
}

#[tokio::test]
async fn fetch_models_sends_bearer_and_converts_the_catalog() {
    let body = r#"{"data":[{"id":"qwen3-local"},{"id":"llama-4-local"}]}"#;
    let (base_url, request_rx) = serve_once(body, "200 OK").await;
    let models = tokio::time::timeout(Duration::from_secs(5), fetch_models(&base_url, Some("sk-test"), "ds4"))
        .await
        .expect("fetch must not hang")
        .expect("catalog fetch succeeds");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].id, "qwen3-local");
    assert_eq!(models[0].base_url, base_url);
    let request = request_rx.await.unwrap();
    assert!(request.starts_with("GET /v1/models "), "{request}");
    assert!(
        request.to_lowercase().contains("authorization: bearer sk-test"),
        "{request}"
    );
}

#[tokio::test]
async fn fetch_models_reports_http_errors() {
    let (base_url, _request_rx) = serve_once("{\"error\":\"nope\"}", "401 Unauthorized").await;
    let err = fetch_models(&base_url, None, "ds4")
        .await
        .expect_err("a non-2xx response must fail");
    assert!(err.contains("HTTP 401"), "{err}");
}

#[tokio::test]
async fn fetch_models_requires_a_base_url() {
    let err = fetch_models("   ", None, "ds4").await.unwrap_err();
    assert!(err.contains("base_url"), "{err}");
}
