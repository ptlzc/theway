use super::super::wire::{
    make_collapse_node, persisted_run_to_wire, storage_node_to_wire, subagent_snapshot_to_wire,
};
use theway_contract::dag::{
    Direction, NodeResult, NodeStatus, PersistedNode, PersistedRun, RunKind,
};
use theway_core::multiagent::session_graph::{SessionGraphState, SubagentJobSnapshot};
use theway_storage::session_graph::SessionGraphNode;
use theway_transport::wire::WireSessionGraphNodeType;

fn persisted_node(status: NodeStatus) -> PersistedNode {
    PersistedNode {
        id: "n1".to_string(),
        agent: "main".to_string(),
        task: "task".to_string(),
        depends_on: vec![],
        timeout: None,
        cwd: None,
        provider: None,
        model: None,
        thinking: None,
        max_iterations: None,
        tools: None,
        status: status.clone(),
        attempt: 0,
        started_at: Some(10),
        completed_at: Some(20),
        error: if status == NodeStatus::Failed {
            Some("boom".to_string())
        } else {
            None
        },
        input_tokens: Some(1),
        output_tokens: Some(2),
        result: Some(NodeResult {
            success: true,
            error: None,
            duration_ms: Some(5),
            attempt: 0,
            total_attempts: 1,
        }),
        output: Some("tail".to_string()),
        live_preview: Some("preview".to_string()),
    }
}

#[test]
fn storage_node_to_wire_maps_all_node_types() {
    let base = SessionGraphNode {
        id: "n".into(),
        node_type: "collapsed".into(),
        parent_id: None,
        child_ids: vec!["child".into()],
        name: "name".into(),
        status: "collapsed".into(),
        summary: Some("summary".into()),
        raw_text_ref: None,
        source_session_id: Some("src".into()),
        run_id: None,
        node_id: None,
        job_id: None,
        subagent_graph: serde_json::json!({}),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: Some("2026-01-02T00:00:00Z".into()),
    };

    let collapsed = storage_node_to_wire(&base, "sess");
    assert_eq!(collapsed.node_type, WireSessionGraphNodeType::Collapsed);
    assert_eq!(collapsed.title, "name");
    assert_eq!(collapsed.summary, "summary");
    assert_eq!(collapsed.parent_node_id, None);
    assert_eq!(collapsed.collapsed_session_id.as_deref(), Some("src"));

    let mut session_node = base.clone();
    session_node.node_type = "session".into();
    assert_eq!(
        storage_node_to_wire(&session_node, "sess").node_type,
        WireSessionGraphNodeType::Session
    );

    let mut other = base;
    other.node_type = "other".into();
    assert_eq!(
        storage_node_to_wire(&other, "sess").node_type,
        WireSessionGraphNodeType::Unspecified
    );
}

#[test]
fn make_collapse_node_builds_graph_node_with_parent_and_state() {
    let state = SessionGraphState::default();
    let node = make_collapse_node(
        "node-1",
        "child-1",
        "source-1",
        "Title",
        "Summary",
        &state,
        Some("parent-1"),
    );
    assert_eq!(node.id, "node-1");
    assert_eq!(node.node_type, "collapsed");
    assert_eq!(node.parent_id.as_deref(), Some("parent-1"));
    assert_eq!(node.name, "Title");
    assert_eq!(node.status, "collapsed");
    assert_eq!(node.summary.as_deref(), Some("Summary"));
    assert_eq!(node.source_session_id.as_deref(), Some("source-1"));
    assert_eq!(node.subagent_graph, serde_json::json!({}));
    assert!(node.updated_at.is_some());
}

#[test]
fn persisted_run_to_wire_maps_status_and_direction() {
    let running = persisted_run("run-1", vec![NodeStatus::Running, NodeStatus::Failed]);
    assert_eq!(persisted_run_to_wire(&running).status, "running");

    let failed = persisted_run("run-2", vec![NodeStatus::Succeeded, NodeStatus::Failed]);
    assert_eq!(persisted_run_to_wire(&failed).status, "failed");

    let pending = persisted_run("run-3", vec![NodeStatus::Pending, NodeStatus::Ready]);
    assert_eq!(persisted_run_to_wire(&pending).status, "running");

    let completed = persisted_run("run-4", vec![NodeStatus::Succeeded, NodeStatus::Skipped]);
    let wire = persisted_run_to_wire(&completed);
    assert_eq!(wire.status, "completed");
    assert_eq!(wire.direction, "TD");
    assert_eq!(wire.error.as_deref(), None);
    assert_eq!(wire.completed_at, Some(20));

    let mut lr = completed;
    lr.direction = Direction::Lr;
    assert_eq!(persisted_run_to_wire(&lr).direction, "LR");
}

fn persisted_run(id: &str, statuses: Vec<NodeStatus>) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        name: "run".into(),
        max_concurrency: 2,
        fail_fast: false,
        direction: Direction::Td,
        created_at: 0,
        session_id: None,
        kind: RunKind::Dag,
        nodes: statuses.into_iter().map(persisted_node).collect(),
    }
}

#[test]
fn subagent_snapshot_to_wire_computes_duration_and_fields() {
    let job = SubagentJobSnapshot {
        id: "job-1".into(),
        agent: "helper".into(),
        source: "test".into(),
        run_id: Some("run-1".into()),
        node_id: Some("node-1".into()),
        session_id: Some("sess".into()),
        status: "running".into(),
        started_at: Some(100),
        completed_at: Some(250),
        attempt: 1,
        total_attempts: 2,
        input_tokens: 10,
        output_tokens: 20,
        chars: 30,
        tools_called: 40,
        turn: 5,
        error: None,
        output_tail: "out".into(),
        truncated: false,
        live_preview: None,
        tps: Some(1.5),
        cps: Some(2.5),
    };
    let wire = subagent_snapshot_to_wire(&job);
    assert_eq!(wire.id, "job-1");
    assert_eq!(wire.duration_ms, Some(150));
    assert_eq!(wire.tps, Some(1.5));
    assert_eq!(wire.input_tokens, Some(10));

    let mut no_duration = job;
    no_duration.completed_at = None;
    let wire = subagent_snapshot_to_wire(&no_duration);
    assert_eq!(wire.duration_ms, None);
    assert_eq!(wire.completed_at, None);
}
