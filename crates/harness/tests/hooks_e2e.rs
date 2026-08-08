//! End-to-end test for user-configured webhook hooks. This drives a real AgentHarness,
//! loads hooks from a THEWAY_DIR-scoped hooks.toml, subscribes the hook listener, and
//! verifies that the agent's turn_end event is delivered as an HTTP POST.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage, StreamFn,
    ThinkingLevel,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[allow(dead_code)]
#[path = "../src/config.rs"]
mod config;
#[allow(dead_code)]
#[path = "../src/hooks.rs"]
mod hooks;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    key: &'static str,
    old: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let old = std::env::var(key).ok();
        // Tests in Rust 2024 require acknowledging that process env is global.
        unsafe { std::env::set_var(key, value) };
        Self { key, old }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(old) = &self.old {
            unsafe { std::env::set_var(self.key, old) };
        } else {
            unsafe { std::env::remove_var(self.key) };
        }
    }
}

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: ModelCost::default(),
        context_window: 1,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

async fn capture_one_request() -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{addr}/theway-hook");
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 1024];
        let header_end = loop {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "client closed before headers were complete");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_headers_end(&buf) {
                break pos;
            }
        };

        let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let content_len = content_length(&headers).unwrap_or(0);
        let body_end = header_end + 4 + content_len;
        while buf.len() < body_end {
            let n = socket.read(&mut chunk).await.unwrap();
            assert!(n > 0, "client closed before body was complete");
            buf.extend_from_slice(&chunk[..n]);
        }

        socket
            .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8_lossy(&buf[..body_end]).into_owned()
    });
    (url, handle)
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_webhook_hook_receives_turn_end_from_agent_harness() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());
    let (webhook_url, request) = capture_one_request().await;

    let hooks_toml = format!(
        r#"
[[hook]]
event = "turn_end"
webhook = "{webhook_url}"
timeout_ms = 3000

[hook.headers]
X-Theway-Harness-Test = "webhook-e2e"
"#
    );
    std::fs::write(theway_dir.path().join("hooks.toml"), hooks_toml).unwrap();

    let model = faux_model();
    let loaded = hooks::load(
        cwd.path(),
        "session-webhook-e2e",
        Some(&model),
        Some(ThinkingLevel::Off),
    )
    .await;
    assert_eq!(loaded.diagnostics, Vec::<String>::new());
    assert_eq!(loaded.runner.len(), 1);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(model, session);
    opts.stream_fn = Some(faux_stream("webhook ack"));
    let harness = AgentHarness::new(opts);
    let _unsub = harness.agent().subscribe(loaded.runner.listener());

    harness.prompt("trigger webhook").await.unwrap();

    let raw_request = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .expect("webhook request timed out")
        .expect("webhook server task failed");

    assert!(
        raw_request.starts_with("POST /theway-hook HTTP/1.1\r\n"),
        "unexpected request line: {raw_request}"
    );
    assert!(
        raw_request
            .to_ascii_lowercase()
            .contains("x-theway-test: webhook-e2e"),
        "custom header missing: {raw_request}"
    );

    let (_, body) = raw_request
        .split_once("\r\n\r\n")
        .expect("request must contain a body separator");
    let payload: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(payload["event"], "turn_end");
    assert_eq!(payload["session_id"], "session-webhook-e2e");
    assert_eq!(payload["cwd"], cwd.path().display().to_string());
    assert_eq!(payload["model_provider"], "faux");
    assert_eq!(payload["model_id"], "faux");
    assert_eq!(payload["thinking_level"], "off");
    assert_eq!(payload["source"], "user");
    assert_eq!(payload["message_kind"], "assistant");
    assert_eq!(payload["message_summary"], "webhook ack");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compaction_webhook_receives_manual_force_compact_from_harness_bus() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());
    let (webhook_url, request) = capture_one_request().await;

    let hooks_toml = format!(
        r#"
[[hook]]
event = "compaction"
webhook = "{webhook_url}"
timeout_ms = 3000
"#
    );
    std::fs::write(theway_dir.path().join("hooks.toml"), hooks_toml).unwrap();

    let model = faux_model();
    let loaded = hooks::load(
        cwd.path(),
        "session-manual-compaction-e2e",
        Some(&model),
        Some(ThinkingLevel::Off),
    )
    .await;
    assert_eq!(loaded.diagnostics, Vec::<String>::new());
    assert_eq!(loaded.runner.len(), 1);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(model, session);
    opts.stream_fn = Some(faux_stream("manual summary"));
    let harness = AgentHarness::new(opts);
    let _unsub_hooks = harness.subscribe_harness(loaded.runner.harness_listener());

    harness.prompt("first turn").await.unwrap();
    harness.prompt("second turn").await.unwrap();
    assert!(harness.force_compact(None).await.unwrap());

    let raw_request = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .expect("manual compaction webhook request timed out")
        .expect("webhook server task failed");
    let (_, body) = raw_request
        .split_once("\r\n\r\n")
        .expect("request must contain a body separator");
    let payload: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(payload["event"], "compaction");
    assert_eq!(payload["session_id"], "session-manual-compaction-e2e");
    assert_eq!(payload["source"], "user");
    assert_eq!(payload["compaction_trigger"], "manual");
    assert_eq!(payload["compaction_summary"], "manual summary");
    assert!(
        payload["compaction_tokens_before"].as_u64().unwrap_or(0) > 0,
        "payload: {payload}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compaction_webhook_receives_auto_compaction_from_harness_bus() {
    let _env_lock = ENV_LOCK.lock().unwrap();
    let theway_dir = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let _theway_dir_guard = EnvGuard::set("THEWAY_DIR", theway_dir.path());
    let (webhook_url, request) = capture_one_request().await;

    let hooks_toml = format!(
        r#"
[[hook]]
event = "compaction"
webhook = "{webhook_url}"
timeout_ms = 3000
"#
    );
    std::fs::write(theway_dir.path().join("hooks.toml"), hooks_toml).unwrap();

    let model = faux_model();
    let loaded = hooks::load(
        cwd.path(),
        "session-auto-compaction-e2e",
        Some(&model),
        Some(ThinkingLevel::Off),
    )
    .await;
    assert_eq!(loaded.diagnostics, Vec::<String>::new());
    assert_eq!(loaded.runner.len(), 1);

    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(model, session);
    opts.stream_fn = Some(faux_stream("auto summary"));
    let harness = AgentHarness::new(opts);
    let _unsub_hooks = harness.subscribe_harness(loaded.runner.harness_listener());

    harness.prompt("first turn").await.unwrap();
    harness.prompt("second turn").await.unwrap();
    harness
        .prompt("third turn triggers auto compaction first")
        .await
        .unwrap();

    let raw_request = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .expect("auto compaction webhook request timed out")
        .expect("webhook server task failed");
    let (_, body) = raw_request
        .split_once("\r\n\r\n")
        .expect("request must contain a body separator");
    let payload: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(payload["event"], "compaction");
    assert_eq!(payload["session_id"], "session-auto-compaction-e2e");
    assert_eq!(payload["source"], "user");
    assert_eq!(payload["compaction_trigger"], "auto");
    assert_eq!(payload["compaction_summary"], "auto summary");
    assert!(
        payload["compaction_tokens_before"].as_u64().unwrap_or(0) > 0,
        "payload: {payload}"
    );
}
