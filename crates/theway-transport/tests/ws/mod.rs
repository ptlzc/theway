//! Tests for `ws` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;

#[test]
fn event_json_matches_wire_shape() {
    let event = AgentJobEvent::Output {
        id: "job-1".into(),
        chunk: "hi".into(),
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_output");
    assert_eq!(value["id"], "job-1");
    assert_eq!(value["chunk"], "hi");

    let event = AgentJobEvent::Completed {
        id: "job-1".into(),
        status: theway_core::multiagent::registry::JobStatus::Succeeded,
        error: None,
        chars: 10,
        tokens_in: 5,
        tokens_out: 3,
        tools_called: 2,
    };
    let value = event_json(&event);
    assert_eq!(value["event"], "subagent_completed");
    assert_eq!(value["status"], "succeeded");
    assert!(value["error"].is_null());
    assert_eq!(value["tools_called"], 2);
}

#[test]
fn dag_event_json_matches_wire_shape() {
    use theway_core::multiagent::graph::types::{DagStatus, NodeStatus};

    let value = dag_event_json(&DagEvent::RunStatus {
        run_id: "goal-1".into(),
        session_id: "sess-1".into(),
        status: DagStatus::Running,
        error: None,
    });
    assert_eq!(value["event"], "run_status");
    assert_eq!(value["run_id"], "goal-1");
    assert_eq!(value["session_id"], "sess-1");
    assert_eq!(value["status"], "running");
    assert!(value["error"].is_null());

    let value = dag_event_json(&DagEvent::NodeStatus {
        run_id: "goal-1".into(),
        session_id: "sess-1".into(),
        node_id: "main".into(),
        status: NodeStatus::Failed,
        error: Some("condition broken".into()),
    });
    assert_eq!(value["event"], "node_status");
    assert_eq!(value["node_id"], "main");
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
