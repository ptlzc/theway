//! Tests for `ws` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::transport::SlashCompleter;
use crate::wire::{
    WireCommand, WireContextUsage, WireDaemonConfig, WireExtensionSnapshot, WirePathContext,
    WireStatus,
};
use axum::extract::ws::Message;
use std::sync::Arc;

#[test]
fn event_json_matches_wire_shape() {
    let event = WireAgentEvent::Started {
        id: "job-1".into(),
        agent: "researcher".into(),
        source: "dag".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
        session_id: "sess-1".into(),
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_started");
    assert_eq!(value["id"], "job-1");
    assert_eq!(value["agent"], "researcher");
    assert_eq!(value["session_id"], "sess-1");

    let event = WireAgentEvent::Output {
        id: "job-1".into(),
        chunk: "hi".into(),
        session_id: "sess-1".into(),
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_output");
    assert_eq!(value["id"], "job-1");
    assert_eq!(value["chunk"], "hi");
    assert_eq!(value["session_id"], "sess-1");

    let event = WireAgentEvent::Metrics {
        id: "job-1".into(),
        tps: Some(12.5),
        cps: None,
        chars: 100,
        tokens_in: 20,
        tokens_out: 30,
        tools_called: 2,
        turn: 1,
        session_id: "sess-1".into(),
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_metrics");
    assert_eq!(value["id"], "job-1");
    assert_eq!(value["session_id"], "sess-1");

    let event = WireAgentEvent::Completed {
        id: "job-1".into(),
        status: "succeeded".into(),
        error: None,
        chars: 10,
        tokens_in: 5,
        tokens_out: 3,
        tools_called: 2,
        session_id: "sess-1".into(),
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_completed");
    assert_eq!(value["status"], "succeeded");
    assert!(value["error"].is_null());
    assert_eq!(value["tools_called"], 2);
    assert_eq!(value["session_id"], "sess-1");
}

#[test]
fn dag_event_json_matches_wire_shape() {
    let value = dag_event_json(&WireDagEvent::RunStatus {
        run_id: "goal-1".into(),
        session_id: "sess-1".into(),
        status: "running".into(),
        error: None,
    });
    assert_eq!(value["event"], "run_status");
    assert_eq!(value["run_id"], "goal-1");
    assert_eq!(value["session_id"], "sess-1");
    assert_eq!(value["status"], "running");
    assert!(value["error"].is_null());

    let value = dag_event_json(&WireDagEvent::NodeStatus {
        run_id: "goal-1".into(),
        session_id: "sess-1".into(),
        node_id: "main".into(),
        status: "failed".into(),
        error: Some("condition broken".into()),
    });
    assert_eq!(value["event"], "node_status");
    assert_eq!(value["node_id"], "main");
    assert_eq!(value["session_id"], "sess-1");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["error"], "condition broken");
}

#[test]
fn client_frames_parse_jsonrpc_requests() {
    // Client frames are JSON-RPC 2.0 requests: {jsonrpc, id, method, params}.
    let v: serde_json::Value =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"send_message","params":{"text":"hi"}}"#)
            .unwrap();
    assert_eq!(v["method"], "send_message");
    assert_eq!(v["params"]["text"], "hi");
    let v: serde_json::Value =
        serde_json::from_str(r#"{"jsonrpc":"2.0","id":2,"method":"abort"}"#).unwrap();
    assert_eq!(v["method"], "abort");
    let v: serde_json::Value = serde_json::from_str(
        r#"{"jsonrpc":"2.0","id":3,"method":"set_model","params":{"model":"anthropic:claude"}}"#,
    )
    .unwrap();
    assert_eq!(v["method"], "set_model");
    let v: serde_json::Value = serde_json::from_str(
        r#"{"jsonrpc":"2.0","id":4,"method":"control_plane_resolve","params":{"approve":true}}"#,
    )
    .unwrap();
    assert_eq!(v["method"], "control_plane_resolve");
    let v: serde_json::Value = serde_json::from_str(
        r#"{"jsonrpc":"2.0","id":5,"method":"get_node_output","params":{"run_id":"r","node_id":"n","offset":3}}"#,
    )
    .unwrap();
    assert_eq!(v["method"], "get_node_output");
}

fn ws_http_state() -> (crate::http::HttpState, tokio::sync::mpsc::UnboundedReceiver<WireCommand>) {
    let (commands, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(crate::testing::FakeSessionOps::new());
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> =
        std::sync::Arc::new(crate::testing::FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(crate::testing::FakeStorageOps::new());
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(WirePathContext::default()));
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(WireDaemonConfig::default()));
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(crate::testing::ChannelCommandOps::new(commands.clone())),
            session_ops.clone(),
            std::sync::Arc::new(crate::UnavailableSessionObservability),
            std::sync::Arc::new(crate::UnavailableGraphOps),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(crate::testing::SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                commands.clone(),
            )),
        ),
    );
    let state = crate::http::HttpState {
        commands,
        snapshots: tokio::sync::broadcast::channel(16).0,
        latest: std::sync::Arc::new(parking_lot::Mutex::new(WireStatus {
            session_id: "sess-1".into(),
            model: "m".into(),
        thinking_level: "off".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: crate::testing::empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: Vec::new(),
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: WireContextUsage::default(),
            session_usage: WireContextUsage::default(),
            tui_max_feed_lines: None,
            extensions: WireExtensionSnapshot::default(),
            system_context: String::new(),
            shell_count: 0,
            observability: Default::default(),
        })),
        session_states: Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
        completer: SlashCompleter::from_commands(Vec::new()),
        events: tokio::sync::broadcast::channel(16).0,
        dag_events: tokio::sync::broadcast::channel(16).0,
        job_ops: std::sync::Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    };
    (state, command_rx)
}

#[tokio::test]
async fn handle_client_frame_ignores_malformed_and_notifications() {
    let (state, _rx) = ws_http_state();
    assert!(handle_client_frame("not-json", &state).await.is_none());
    assert!(handle_client_frame(r#"{"jsonrpc":"2.0","method":"ping"}"#, &state).await.is_none());
}

#[tokio::test]
async fn handle_client_frame_replies_with_rpc_errors() {
    let (state, _rx) = ws_http_state();
    let reply = handle_client_frame(
        r#"{"jsonrpc":"2.0","id":1,"method":"no_such_method","params":{}}"#,
        &state,
    )
    .await
    .expect("reply");
    match reply {
        Message::Text(text) => {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value["error"]["code"], -32601);
        }
        other => panic!("expected text reply, got {other:?}"),
    }
}
