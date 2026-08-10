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
        status: theway_core::runtime::multiagent::registry::JobStatus::Succeeded,
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
    use theway_core::runtime::multiagent::graph::types::{DagStatus, NodeStatus};

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
fn client_frames_parse_tagged() {
    let frame: ClientFrame = serde_json::from_str(r#"{"type":"prompt","text":"hi"}"#).unwrap();
    match frame {
        ClientFrame::Prompt { text, images } => {
            assert_eq!(text, "hi");
            assert!(images.is_empty());
        }
        other => panic!("unexpected: {other:?}"),
    }
    let frame: ClientFrame = serde_json::from_str(r#"{"type":"abort"}"#).unwrap();
    assert!(matches!(frame, ClientFrame::Abort));
    let frame: ClientFrame =
        serde_json::from_str(r#"{"type":"set_model","spec":"anthropic:claude"}"#).unwrap();
    assert!(matches!(frame, ClientFrame::SetModel { .. }));
    let frame: ClientFrame =
        serde_json::from_str(r#"{"type":"resolve_control_plane","approve":true}"#).unwrap();
    assert!(matches!(frame, ClientFrame::ResolveControlPlane { .. }));
    let frame: ClientFrame =
        serde_json::from_str(r#"{"type":"get_node_output","run_id":"r","node_id":"n","offset":3}"#)
            .unwrap();
    assert!(matches!(frame, ClientFrame::GetNodeOutput { .. }));
    let frame: ClientFrame = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
    assert!(matches!(frame, ClientFrame::Ping));
}
