//! Tests for `web_search` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serializes tests that read/write `BRAVE_SEARCH_API_KEY`. The env var is process-global;
/// e2e tests live in their own process, so this lock only needs to cover the lib target.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvGuard;

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("BRAVE_SEARCH_API_KEY") };
    }
}

fn set_api_key() -> EnvGuard {
    unsafe { std::env::set_var("BRAVE_SEARCH_API_KEY", "test-token") };
    EnvGuard
}

/// Spawn a tiny HTTP/1.1 server that answers a single request with `body`.
async fn spawn_mock(status: u16, body: String) -> (String, tokio::task::JoinHandle<()>) {
    spawn_mock_with(status, body, None).await
}

/// Spawn a tiny HTTP/1.1 server and, when `requests` is provided, record the raw
/// request bytes (usually enough to inspect the query string).
async fn spawn_mock_with(
    status: u16,
    body: String,
    requests: Option<Arc<tokio::sync::Mutex<Vec<String>>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/search");
    let reason = if status == 200 { "OK" } else { "Error" };
    let handle = tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            if let Some(reqs) = &requests {
                reqs.lock()
                    .await
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
            }
            let resp = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (url, handle)
}

#[test]
fn definition_lists_query_as_required() {
    let def = WebSearchTool::new().definition().clone();
    let req = def
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .unwrap();
    assert!(req.iter().any(|v| v.as_str() == Some("query")));
}

#[test]
fn default_base_url_label_and_execution_mode_are_stable() {
    let tool = WebSearchTool::default();

    assert_eq!(
        tool.base_url,
        "https://api.search.brave.com/res/v1/web/search"
    );
    assert_eq!(tool.label(), "web_search");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
}

#[tokio::test]
async fn execute_missing_query_reports_missing_arg() {
    let tool = WebSearchTool::with_base_url("http://127.0.0.1:9/search".into());

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({}),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("missing required arg: query"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_missing_api_key_reports_not_configured() {
    let _guard = ENV_LOCK.lock().await;
    unsafe { std::env::remove_var("BRAVE_SEARCH_API_KEY") };

    let tool = WebSearchTool::with_base_url("http://127.0.0.1:9/search".into());

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust async" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err();

    assert!(
        err.to_string().contains("BRAVE_SEARCH_API_KEY"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_renders_brave_results() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let payload = r#"{
        "web": {
            "results": [
                {"title":"Rust","url":"https://rust-lang.org","description":"safe systems language"},
                {"title":"tokio","url":"https://tokio.rs","description":"async runtime"}
            ]
        }
    }"#;
    let (url, _server) = spawn_mock(200, payload.to_string()).await;
    let tool = WebSearchTool::with_base_url(url);

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
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(body.contains("Rust"), "title 1: {body}");
    assert!(body.contains("https://rust-lang.org"), "url 1: {body}");
    assert!(body.contains("safe systems language"), "desc 1: {body}");
    assert!(body.contains("tokio"), "title 2: {body}");
    assert_eq!(res.details["results"], serde_json::json!(2));
}

#[tokio::test]
async fn execute_clamps_count_to_max() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let payload = r#"{"web":{"results":[{"title":"Rust","url":"https://rust-lang.org","description":"d"}]}}"#;
    let (url, server) = spawn_mock_with(200, payload.to_string(), Some(requests.clone())).await;
    let tool = WebSearchTool::with_base_url(url);

    let res = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust", "count": 100 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(res.details["results"], serde_json::json!(1));
    let captured = requests.lock().await.join("\n");
    assert!(captured.contains("count=20"), "got: {captured}");
    assert!(!captured.contains("count=100"), "got: {captured}");
    server.await.unwrap();
}

#[tokio::test]
async fn execute_defaults_count_to_ten() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let requests = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    // A completely empty JSON object also exercises the "no `web` key" branch.
    let payload = "{}";
    let (url, server) = spawn_mock_with(200, payload.to_string(), Some(requests.clone())).await;
    let tool = WebSearchTool::with_base_url(url);

    let res = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

    assert_eq!(res.details["results"], serde_json::json!(0));
    let captured = requests.lock().await.join("\n");
    assert!(captured.contains("count=10"), "got: {captured}");
    server.await.unwrap();
}

#[tokio::test]
async fn execute_non_success_status_reports_backend_error() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let (url, _server) = spawn_mock(500, "boom".to_string()).await;
    let tool = WebSearchTool::with_base_url(url);

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("search backend status 500"), "got: {err}");
    assert!(err.contains("boom"), "got: {err}");
}

#[tokio::test]
async fn execute_malformed_json_reports_parse_error() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let (url, _server) = spawn_mock(200, "not json".to_string()).await;
    let tool = WebSearchTool::with_base_url(url);

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("parse response"), "got: {err}");
}

#[tokio::test]
async fn execute_empty_results_returns_no_results() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let (url, _server) = spawn_mock(200, r#"{"web":{"results":[]}}"#.to_string()).await;
    let tool = WebSearchTool::with_base_url(url);

    let res = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

    let body = match &res.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(body.contains("no results for query: rust"), "got: {body}");
    assert_eq!(res.details["query"], serde_json::json!("rust"));
    assert_eq!(res.details["results"], serde_json::json!(0));
}

#[tokio::test]
async fn execute_missing_result_fields_uses_placeholders() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let (url, _server) = spawn_mock(200, r#"{"web":{"results":[{}]}}"#.to_string()).await;
    let tool = WebSearchTool::with_base_url(url);

    let res = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

    let body = match &res.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert!(body.contains("(no title)"), "got: {body}");
    assert!(body.contains("(no url)"), "got: {body}");
}

#[tokio::test]
async fn execute_cancelled_before_send_returns_cancelled() {
    let _guard = ENV_LOCK.lock().await;
    let _env = set_api_key();

    let token = CancellationToken::new();
    token.cancel();
    let tool = WebSearchTool::with_base_url("http://127.0.0.1:9/search".into());

    let err = tool
        .execute(
            "call-1",
            serde_json::json!({ "query": "rust" }),
            token,
            None,
        )
        .await
        .unwrap_err()
        .to_string();

    assert!(err.contains("cancelled"), "got: {err}");
}
