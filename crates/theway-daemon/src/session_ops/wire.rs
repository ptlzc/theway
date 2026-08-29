//! Storage → wire conversion helpers for session graph snapshots.

use theway_storage::session_graph::SessionGraphNode;
use theway_transport::wire::{
    WireAgentJobSnapshot, WireDagNodeSnapshot, WireDagRunSnapshot, WireNodeResultSnapshot,
    WireSessionGraphNode, WireSessionGraphNodeType,
};

use super::metadata::now_rfc3339;

pub(super) fn storage_node_to_wire(
    node: &SessionGraphNode,
    session_id: &str,
) -> WireSessionGraphNode {
    WireSessionGraphNode {
        id: node.id.clone(),
        session_id: session_id.to_string(),
        node_type: match node.node_type.as_str() {
            "collapsed" => WireSessionGraphNodeType::Collapsed,
            "session" => WireSessionGraphNodeType::Session,
            _ => WireSessionGraphNodeType::Unspecified,
        },
        title: node.name.clone(),
        summary: node.summary.clone().unwrap_or_default(),
        parent_node_id: node.parent_id.clone(),
        child_node_ids: node.child_ids.clone(),
        collapsed_session_id: node.source_session_id.clone(),
        collapsed_at: Some(node.created_at.clone()),
        created_at: Some(node.created_at.clone()),
        updated_at: node.updated_at.clone(),
        message_count: 0,
    }
}

pub(super) fn make_collapse_node(
    node_id: &str,
    _child_session_id: &str,
    source_session_id: &str,
    title: &str,
    summary: &str,
    graph_state: &theway_core::multiagent::session_graph::SessionGraphState,
    parent_id: Option<&str>,
) -> SessionGraphNode {
    let now = now_rfc3339();
    SessionGraphNode {
        id: node_id.to_string(),
        node_type: "collapsed".to_string(),
        parent_id: parent_id.map(str::to_string),
        name: title.to_string(),
        status: "collapsed".to_string(),
        summary: Some(summary.to_string()),
        raw_text_ref: Some(source_session_id.to_string()),
        source_session_id: Some(source_session_id.to_string()),
        run_id: None,
        node_id: None,
        job_id: None,
        subagent_graph: serde_json::to_value(graph_state).unwrap_or_default(),
        child_ids: Vec::new(),
        created_at: now.clone(),
        updated_at: Some(now),
    }
}

pub(super) fn persisted_run_to_wire(
    run: &theway_contract::dag::PersistedRun,
) -> WireDagRunSnapshot {
    let status = if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Running)
    {
        "running"
    } else if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Failed)
    {
        "failed"
    } else if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Pending)
    {
        "running"
    } else {
        "completed"
    };
    WireDagRunSnapshot {
        id: run.id.clone(),
        name: run.name.clone(),
        kind: run.kind.as_str().to_string(),
        status: status.to_string(),
        fail_fast: run.fail_fast,
        max_concurrency: run.max_concurrency,
        direction: match run.direction {
            theway_contract::dag::Direction::Td => "TD".to_string(),
            theway_contract::dag::Direction::Lr => "LR".to_string(),
        },
        created_at: run.created_at,
        completed_at: run.nodes.iter().filter_map(|n| n.completed_at).max(),
        error: run.nodes.iter().find_map(|n| n.error.clone()),
        nodes: run.nodes.iter().map(persisted_node_to_wire).collect(),
    }
}

fn persisted_node_to_wire(node: &theway_contract::dag::PersistedNode) -> WireDagNodeSnapshot {
    WireDagNodeSnapshot {
        id: node.id.clone(),
        agent: node.agent.clone(),
        status: match node.status {
            theway_contract::dag::NodeStatus::Pending => "pending",
            theway_contract::dag::NodeStatus::Ready => "ready",
            theway_contract::dag::NodeStatus::Running => "running",
            theway_contract::dag::NodeStatus::Succeeded => "succeeded",
            theway_contract::dag::NodeStatus::Failed => "failed",
            theway_contract::dag::NodeStatus::Skipped => "skipped",
            theway_contract::dag::NodeStatus::Cancelled => "cancelled",
        }
        .to_string(),
        depends_on: node.depends_on.clone(),
        job_id: None,
        attempt: node.attempt,
        started_at: node.started_at,
        completed_at: node.completed_at,
        error: node.error.clone(),
        input_tokens: node.input_tokens,
        output_tokens: node.output_tokens,
        result: node.result.as_ref().map(|r| WireNodeResultSnapshot {
            success: r.success,
            error: r.error.clone(),
            duration_ms: r.duration_ms,
            attempt: r.attempt,
            total_attempts: r.total_attempts,
        }),
        output_tail: node.output.clone(),
        live_preview: node.live_preview.clone(),
    }
}

pub(super) fn subagent_snapshot_to_wire(
    job: &theway_core::SubagentJobSnapshot,
) -> WireAgentJobSnapshot {
    WireAgentJobSnapshot {
        id: job.id.clone(),
        agent: job.agent.clone(),
        source: job.source.clone(),
        run_id: job.run_id.clone(),
        node_id: job.node_id.clone(),
        status: job.status.clone(),
        started_at: job.started_at,
        completed_at: job.completed_at,
        duration_ms: job
            .completed_at
            .zip(job.started_at)
            .map(|(end, start)| (end - start).max(0) as u64),
        attempt: job.attempt,
        total_attempts: job.total_attempts,
        input_tokens: Some(job.input_tokens),
        output_tokens: Some(job.output_tokens),
        error: job.error.clone(),
        output_tail: Some(job.output_tail.clone()),
        live_preview: job.live_preview.clone(),
        tps: job.tps,
        cps: job.cps,
        chars: Some(job.chars),
        tools_called: Some(job.tools_called),
        turn: Some(job.turn),
    }
}
