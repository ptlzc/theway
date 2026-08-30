/// Session graph node type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireSessionGraphNodeType {
    #[default]
    Unspecified,
    Session,
    Collapsed,
}

/// A node in the session graph.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSessionGraphNode {
    pub id: String,
    pub session_id: String,
    pub node_type: WireSessionGraphNodeType,
    pub title: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_node_id: Option<String>,
    #[serde(default)]
    pub child_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    pub message_count: u32,
}

/// Detailed record for a collapsed session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireCollapsedSessionNode {
    pub node_id: String,
    pub session_id: String,
    pub title: String,
    pub summary: String,
    pub message_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_into_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_into_node_id: Option<String>,
    #[serde(default)]
    pub original_session_ids: Vec<String>,
}

/// CollapseSession RPC wire request/response.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireCollapseSessionRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireCollapseSessionResponse {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<WireSessionGraphNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<WireCollapsedSessionNode>,
}

/// Streaming frame for a session graph node.
#[derive(Clone, Debug, PartialEq)]
pub enum WireSessionGraphNodeStreamFrame {
    Node(WireSessionGraphNode),
    Block(crate::feed::WireFeedBlock),
}

/// graph mode: one DAG run (mirrors `crates/theway-transport/proto/graph_engine.proto` DagRunSnapshot; task text is
/// deliberately excluded from the wire model — full text goes through GetNodeOutput).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireDagRunSnapshot {
    pub id: String,
    pub name: String,
    /// "dag" | "goal" — goal runs are single-node self-loops (condition-terminated).
    pub kind: String,
    pub status: String,
    pub fail_fast: bool,
    pub max_concurrency: usize,
    pub direction: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub nodes: Vec<WireDagNodeSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireDagNodeSnapshot {
    pub id: String,
    pub agent: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub job_id: Option<String>,
    pub attempt: u32,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub result: Option<WireNodeResultSnapshot>,
    pub output_tail: Option<String>,
    pub live_preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireNodeResultSnapshot {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
}

/// Node transcript/output returned by the transport-side
/// [`crate::transport::JobOps`] seam.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireNodeOutput {
    /// `None` means no live or retained job exists for this run/node pair.
    pub output: Option<String>,
    pub truncated: bool,
    pub messages: Option<Vec<serde_json::Value>>,
    pub messages_truncated: bool,
}

/// Portable graph checkpoint returned by the transport-side
/// [`crate::transport::GraphOps`] seam.
#[derive(Clone, Debug, PartialEq)]
pub struct WireGraphCheckpoint {
    pub kind: WireGraphKind,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireGraphKind {
    Dag,
    Goal,
}

/// Subagent event already projected into protocol-owned values.
#[derive(Clone, Debug, PartialEq)]
pub enum WireAgentEvent {
    Started {
        id: String,
        agent: String,
        source: String,
        run_id: Option<String>,
        node_id: Option<String>,
        session_id: String,
    },
    Output {
        id: String,
        chunk: String,
        session_id: String,
    },
    Metrics {
        id: String,
        tps: Option<f64>,
        cps: Option<f64>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
        turn: u32,
        session_id: String,
    },
    Completed {
        id: String,
        status: String,
        error: Option<String>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
        session_id: String,
    },
}

/// DAG event already projected into protocol-owned string statuses.
#[derive(Clone, Debug, PartialEq)]
pub enum WireDagEvent {
    NodeStatus {
        run_id: String,
        session_id: String,
        node_id: String,
        status: String,
        error: Option<String>,
    },
    RunStatus {
        run_id: String,
        session_id: String,
        status: String,
        error: Option<String>,
    },
}

/// Graph mode: one subagent job projected by the host's job adapter (mirrors
/// `crates/theway-transport/proto/graph_engine.proto` SubagentJobSnapshot).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireAgentJobSnapshot {
    pub id: String,
    pub agent: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub status: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error: Option<String>,
    pub output_tail: Option<String>,
    pub live_preview: Option<String>,
    pub tps: Option<f64>,
    pub cps: Option<f64>,
    pub chars: Option<u64>,
    pub tools_called: Option<u64>,
    pub turn: Option<u32>,
}
