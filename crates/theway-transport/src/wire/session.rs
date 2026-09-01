/// session-resource-model: one session as a managed resource (mirrors
/// `crates/theway-transport/proto/session.proto` SessionSummary). Produced by the app-side SessionOps
/// by the host's [`crate::transport::SessionOps`] implementation; served
/// verbatim on JSON and mapped onto the protobuf message by this crate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    pub model: String,
    pub created_at: String,
    /// Deprecated epoch milliseconds; prefer `last_activity_at_rfc3339`.
    pub last_activity_at: i64,
    /// RFC3339 / ISO-8601 with offset (UTC), null when absent.
    pub last_activity_at_rfc3339: Option<String>,
    pub graph_count: u32,
    pub active_graph_count: u32,
    pub busy: bool,
    pub preview: Option<String>,
    /// Pi-style tree prefix (`├─ ` / `└─ ` / `│ `) for fork-lineage display.
    pub tree_prefix: String,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

/// Convert epoch milliseconds to an RFC3339 / ISO-8601 UTC string.
pub fn epoch_millis_to_rfc3339(millis: i64) -> Option<String> {
    let secs = millis.div_euclid(1000);
    let nanos = millis.rem_euclid(1000) as u32 * 1_000_000;
    Utc.timestamp_opt(secs, nanos)
        .earliest()
        .map(|dt| dt.to_rfc3339())
}

/// A resolved model reference in a session runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireModelRef {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// Session identity/display metadata.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSessionInfo {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub created_at: String,
    /// Deprecated epoch milliseconds; prefer `last_activity_at_rfc3339`.
    pub last_activity_at: i64,
    pub last_activity_at_rfc3339: Option<String>,
    pub busy: bool,
    pub preview: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub graph_count: u32,
    pub active_graph_count: u32,
    pub queued_count: usize,
    pub sidebar: WireSidebarSnapshot,
}

/// Live session runtime/context.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSessionRuntime {
    pub model: WireModelRef,
    pub thinking_level: String,
    #[serde(default)]
    pub supported_thinking_levels: Vec<String>,
    #[serde(default)]
    pub context_usage: WireContextUsage,
    #[serde(default)]
    pub session_context_usage: WireContextUsage,
    pub tui_max_feed_lines: Option<u64>,
    /// Number of background shells still alive (registered and not yet
    /// exited) across the daemon process. Mirrors `WireStatus::shell_count`;
    /// carried through this nested snapshot so the gRPC/HTTP session snapshot
    /// round-trip preserves it for the TUI `[n shell]` counter.
    #[serde(default)]
    pub shell_count: u64,
    #[serde(default)]
    pub model_catalog: Vec<ProviderGroup>,
    pub latest_trigger_poll: Option<crate::feed::TriggerPollStatus>,
    pub goal: Option<WireGoalSnapshot>,
    pub control_plane_prompt: Option<WireControlPlanePromptSnapshot>,
    #[serde(default, skip_serializing_if = "WireExtensionSnapshot::is_empty")]
    pub extensions: WireExtensionSnapshot,
    /// Full rendered system context for the next request (base prompt + skills
    /// + tool inventory + working directory + memory + lineage).
    ///
    /// Mirrors the request/header epoch snapshot in deepseek-harness session
    /// logs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub system_context: String,
}

/// Transcript plane of a session snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSessionFeed {
    #[serde(default)]
    pub blocks: Vec<crate::feed::WireFeedBlock>,
    #[serde(default)]
    pub lines: Vec<String>,
    #[serde(default)]
    pub blocks_base: u64,
    #[serde(default)]
    pub lines_base: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block_patches: Vec<WireFeedBlockPatch>,
}

/// Graph-mode state mounted under a session snapshot.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSessionGraphState {
    #[serde(default)]
    pub dags: Vec<WireDagRunSnapshot>,
    #[serde(default)]
    pub subagents: Vec<WireAgentJobSnapshot>,
    #[serde(default)]
    pub nodes: Vec<WireSessionGraphNode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_node_id: Option<String>,
}

/// Session lineage for fork/collapse ancestry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireSessionLineage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_session_id: Option<String>,
    #[serde(default)]
    pub ancestor_session_ids: Vec<String>,
    #[serde(default)]
    pub child_session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_from_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed_into_session_id: Option<String>,
}

/// Full nested session snapshot: the successor of `WireStatus`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireSessionSnapshot {
    pub session_id: String,
    pub info: WireSessionInfo,
    pub runtime: WireSessionRuntime,
    pub feed: WireSessionFeed,
    pub graph_state: WireSessionGraphState,
    pub lineage: WireSessionLineage,
}
