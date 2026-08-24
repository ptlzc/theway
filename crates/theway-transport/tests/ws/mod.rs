//! Tests for `ws` — split out of src (see docs/rust-test-files.md).

use super::*;

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
