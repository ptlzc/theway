//! End-to-end test for the web_search tool against a hand-rolled local HTTP server that
//! mimics Brave Search's JSON shape.

use std::sync::Arc;

use theway_core::AgentTool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[path = "../../core/src/tools/web_search.rs"]
mod web_search;

async fn spawn_mock(json: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}/search", addr);
    let handle = tokio::spawn(async move {
        // Accept a few connections to handle retries / multiple test calls.
        for _ in 0..4 {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    json.len(),
                    json
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        }
    });
    (url, handle)
}

#[tokio::test]
async fn web_search_renders_brave_results() {
    let payload = r#"{
        "web": {
            "results": [
                {"title":"Rust","url":"https://rust-lang.org","description":"safe systems language"},
                {"title":"tokio","url":"https://tokio.rs","description":"async runtime"}
            ]
        }
    }"#;
    let (url, _server) = spawn_mock(payload).await;
    let tool = web_search::WebSearchTool::with_base_url(url);

    // Inject the API key needed by the tool.
    // SAFETY: tests run single-threaded enough that this env var write is fine.
    unsafe { std::env::set_var("BRAVE_SEARCH_API_KEY", "test-token") };

    let res = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust async", "count": 2 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let body = match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(body.contains("Rust"), "title 1: {body}");
    assert!(body.contains("https://rust-lang.org"), "url 1: {body}");
    assert!(body.contains("tokio"), "title 2: {body}");
    let details: &serde_json::Value = &res.details;
    assert_eq!(details.get("results").and_then(|v| v.as_u64()), Some(2));
    drop(Arc::new(()));
    unsafe { std::env::remove_var("BRAVE_SEARCH_API_KEY") };
}

// The "missing API key" path is covered by code review + the explicit error message in
// execute(). We don't ship a test for it because env vars are global to the process and
// races with the success test above. (cargo --test-threads=1 would work but adding that
// requirement per file is uglier than the value provided.)
