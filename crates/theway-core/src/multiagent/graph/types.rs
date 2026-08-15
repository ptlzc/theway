//! DAG orchestration graph types. 1:1 port of the dag-orchestrator extension's
//! `types.ts` (pi-src/extensions/dag-orchestrator).

use serde::{Deserialize, Serialize};
use strum::IntoStaticStr;

/// Lifecycle of a single DAG node.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    /// Dependencies not all done yet.
    Pending,
    /// All deps succeeded/skipped, waiting for a concurrency slot.
    Ready,
    /// Subagent job in flight.
    Running,
    /// Job completed successfully.
    Succeeded,
    /// Job failed.
    Failed,
    /// Orchestrator skipped it (counts as success for downstream).
    Skipped,
    /// Blocked by a failed dep, or the run was cancelled.
    Cancelled,
}

/// Overall run lifecycle.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DagStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Run flavour. Goal runs are single-node self-loops driven by the goal.rs
/// hook (no DAG edges; termination is condition-based, not dependency-based).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize, IntoStaticStr)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum RunKind {
    #[default]
    Dag,
    Goal,
}

impl RunKind {
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

/// Mermaid graph direction.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    #[serde(rename = "TD")]
    Td,
    #[serde(rename = "LR")]
    Lr,
}

/// User-declared node (before validation / runtime state).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagNodeDef {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub depends_on: Option<Vec<String>>,
    /// Idle timeout override (sec), passed through to the subagent runner.
    pub timeout: Option<u64>,
    /// Working directory for the subagent (absolute path). When set, the node's task
    /// prompt pins it and the subagent is told to run all commands from there.
    pub cwd: Option<String>,
    /// Primary-target model override, passed through to the subagent runner.
    pub model: Option<String>,
    /// Primary-target thinking override.
    pub thinking: Option<String>,
    /// Iteration-budget override (LLM-turn attempts); the launcher applies it
    /// over the spec default when set.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Tool allowlist (tool names); `None` means the full resolved tool set.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
}

/// User-declared run definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagRunDef {
    pub name: String,
    pub nodes: Vec<DagNodeDef>,
    /// Max concurrently running nodes. Default 10.
    pub max_concurrency: Option<usize>,
    /// true: any failure aborts the whole run. false (default): only the
    /// failed node's downstream closure is cancelled; independent branches
    /// keep running.
    pub fail_fast: Option<bool>,
    /// Mermaid graph direction. Default TD.
    pub direction: Option<Direction>,
}

/// Lightweight result summary (full output lives in the BgJob registry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeResult {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
}

/// A node with runtime state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagNode {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub depends_on: Vec<String>,
    pub timeout: Option<u64>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    /// Iteration-budget override carried from the definition; persisted so a
    /// restored node still launches with it.
    #[serde(default)]
    pub max_iterations: Option<u32>,
    /// Tool allowlist carried from the definition; persisted like the budget.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    pub status: NodeStatus,
    /// Subagent BgJob id while running / after completion.
    pub job_id: Option<String>,
    /// Completed attempts so far (from the subagent result's total_attempts).
    pub attempt: u32,
    /// Launch generation: incremented every time the node is dispatched by the
    /// scheduler (`start_node`). Callbacks from stale jobs (pre-cancel/pre-retry)
    /// carry their captured generation and are dropped when it no longer matches —
    /// `on_node_update` from an old attempt must never pollute a re-launched one.
    /// Monotonic across retries; not persisted (starts at 0 after a restore).
    #[serde(default)]
    pub launch_gen: u64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// Failure / skip / cancel reason.
    pub error: Option<String>,
    /// Cumulative input tokens reported by the LLM API (live while running,
    /// final once the job completes).
    pub input_tokens: Option<u64>,
    /// Cumulative output tokens reported by the LLM API.
    pub output_tokens: Option<u64>,
    pub result: Option<NodeResult>,
    /// Final output tail (launcher writes, capped ~8 KB).
    pub output: Option<String>,
    /// Live output while running (launcher updates, capped ~2 KB).
    pub live_preview: Option<String>,
    /// Heartbeat (ms): refreshed by the engine on every token/preview update. Lets
    /// orchestrators spot stalled nodes (heartbeat frozen across inspections).
    pub last_active_at: Option<i64>,
}

/// One DAG run. Nodes keep declaration order (graphs are small; linear scan).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DagRun {
    pub id: String,
    pub name: String,
    pub nodes: Vec<DagNode>,
    pub status: DagStatus,
    /// goal runs are single-node self-loops (RunKind::Goal).
    pub kind: RunKind,
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub direction: Direction,
    pub created_at: i64,
    /// Owning pi session id. Runs are session-scoped: dag_* tools refuse to
    /// touch runs owned by another session (multiple concurrent agents).
    pub session_id: Option<String>,
    pub completed_at: Option<i64>,
    /// Epoch ms of last engine activity (state change or node job output).
    /// Used by dag_wait's idle watchdog.
    pub last_activity_at: i64,
    /// Cancellation / failure reason.
    pub error: Option<String>,
}

impl DagRun {
    pub fn node(&self, id: &str) -> Option<&DagNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut DagNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

/// Event-plane message broadcast on engine state changes (node_status /
/// run_status frames in the proto; P3 gap — previously defined, never sent).
#[derive(Clone, Debug)]
pub enum DagEvent {
    NodeStatus {
        run_id: String,
        /// Owning session of the run (empty string when the run is
        /// session-less; `DagRun.session_id` is `Option<String>`).
        session_id: String,
        node_id: String,
        status: NodeStatus,
        error: Option<String>,
    },
    RunStatus {
        run_id: String,
        /// Owning session of the run (empty string when the run is
        /// session-less; `DagRun.session_id` is `Option<String>`).
        session_id: String,
        status: DagStatus,
        error: Option<String>,
    },
}
