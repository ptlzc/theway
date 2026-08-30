//! Unit tests for HTTP transport helpers: the loopback bind policy.
//!
//! App-side helper tests (feed-line projection, prompt image decoding) live in
//! `crate::ui::web_loop` (they exercise App-owned functions).

use super::super::*;
use serde_json::json;

/// Router wired with throwaway transport channels, for endpoint-level tests
/// that only need a snapshot fixture.
pub(crate) fn test_router(latest: WireStatus) -> Router {
    let (command_tx, _) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(crate::testing::FakeSessionOps::new());
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> =
        std::sync::Arc::new(crate::testing::FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WirePathContext::default(),
    ));
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WireDaemonConfig::default(),
    ));
    let session_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let latest = Arc::new(Mutex::new(latest));
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::testing::ChannelCommandOps::new(command_tx.clone())),
            session_ops.clone(),
            std::sync::Arc::new(crate::testing::LiveSessionObservability::new(
                session_ops.clone(),
                session_states.clone(),
                latest.clone(),
                latest.lock().session_id.clone(),
            )),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::testing::SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                command_tx.clone(),
            )),
        ),
    );
    web_router(HttpState {
        commands: command_tx,
        snapshots: snapshot_tx,
        latest,
        session_states,
        completer: SlashCompleter::from_commands(vec!["/help".into(), "/model".into(), "/goal".into()]),
        events: broadcast::channel::<crate::wire::WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<crate::wire::WireDagEvent>(16).0,
        job_ops: std::sync::Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    })
}

/// JSON-RPC 2.0 call helper: POST /rpc, returns the `result` (panics on error).
pub(crate) async fn rpc_call(
    client: &reqwest::Client,
    base: &str,
    id: u64,
    method: &str,
    params: Option<serde_json::Value>,
) -> serde_json::Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params.unwrap_or_else(|| serde_json::Value::Null),
    });
    let resp: serde_json::Value = client
        .post(format!("{base}/rpc"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    if let Some(err) = resp.get("error") {
        panic!("rpc {method} error: {err}");
    }
    resp["result"].clone()
}

/// JSON-RPC call expecting an error; returns `(code, message)`.
pub(crate) async fn rpc_error(
    client: &reqwest::Client,
    base: &str,
    id: u64,
    method: &str,
    params: Option<serde_json::Value>,
) -> (i64, String) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params.unwrap_or_else(|| serde_json::Value::Null),
    });
    let resp: serde_json::Value = client
        .post(format!("{base}/rpc"))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let err = resp["error"].clone();
    assert!(!err.is_null(), "expected rpc error for {method}, got {resp}");
    (
        err["code"].as_i64().unwrap(),
        err["message"].as_str().unwrap_or("").to_string(),
    )
}

#[test]
fn bind_addr_rejects_remote_by_default() {
    let err = bind_addr("0.0.0.0", 0).unwrap_err().to_string();
    assert!(err.contains("refusing non-loopback"));
}

#[test]
fn bind_addr_accepts_loopback_and_localhost() {
    let local = bind_addr("127.0.0.1", 0).unwrap();
    assert!(local.ip().is_loopback());

    let named = bind_addr("localhost", 0).unwrap();
    assert!(named.ip().is_loopback());
}