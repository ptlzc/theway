//! Wire protocol model shared by the `--web` (axum) and `--grpc` (tonic) transport
//! servers: the command enum both event loops consume and the status payload both
//! serialize. Decoupled from the terminal UI — the servers live in the
//! `transport` module (`theway` crate, `server` feature), the event loop stays in
//! `crate::ui::web_loop`. The proto codecs that map these models onto the
//! generated gRPC types live in `transport::proto` as well.

use serde::{Deserialize, Serialize};

use theway_core::runtime::graph_engineering::types::{
    DagNode, DagRun, Direction, NodeResult, NodeStatus,
};

#[derive(Clone, Debug)]
pub struct WebOptions {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub enum WebCommand {
    Submit {
        text: String,
        images: Vec<WebPromptImage>,
        /// true = stop the current turn and run this message now (INTERRUPT);
        /// false = queue after the current turn (GUIDE, default).
        interrupt: bool,
    },
    TriggerRuleNow {
        id: String,
    },
    Abort,
    ResolveControlPlane {
        approve: bool,
    },
    SetModel {
        spec: String,
    },
    /// session-resource-model: switch the runtime to another session (resume semantics).
    /// `CreateSession`'s "make current" path also flows through this command — creating the
    /// session is a sync `SessionOps` call, becoming current goes through the serialized
    /// event loop.
    SwitchSession {
        id: String,
    },
}

/// session-resource-model: one session as a managed resource (mirrors
/// `theway_grpc.proto` SessionSummary). Produced by the app-side SessionOps
/// from the JsonlSessionRepo plus live DagEngine state; served verbatim on
/// the JSON surface and mapped onto the proto message by `theway-server`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    pub model: String,
    pub created_at: String,
    pub last_activity_at: i64,
    pub graph_count: u32,
    pub active_graph_count: u32,
    pub busy: bool,
    pub preview: Option<String>,
}

/// graph mode: one DAG run (mirrors `theway_grpc.proto` DagRunSnapshot; task text is
/// deliberately excluded from the wire model — full text goes through GetNodeOutput).
#[derive(Clone, Debug, Serialize)]
pub struct WebDagRunSnapshot {
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
    pub nodes: Vec<WebDagNodeSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebDagNodeSnapshot {
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
    pub result: Option<WebNodeResultSnapshot>,
    pub output_tail: Option<String>,
    pub live_preview: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebNodeResultSnapshot {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
}

/// graph mode: one subagent job (mirrors `theway_grpc.proto` SubagentJobSnapshot).
/// Populated from the SubagentJobRegistry in P2; the type ships now so the wire
/// shape is stable.
#[derive(Clone, Debug, Serialize)]
pub struct WebSubagentJobSnapshot {
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

#[derive(Clone, Debug, Serialize)]
pub struct WebStatus {
    pub session_id: String,
    pub model: String,
    pub model_catalog: Vec<crate::model_picker::ProviderGroup>,
    pub cwd: String,
    pub busy: bool,
    pub queued_count: usize,
    pub latest_trigger_poll: Option<crate::ui::feed::TriggerPollStatus>,
    pub goal: Option<WebGoalSnapshot>,
    pub control_plane_prompt: Option<WebControlPlanePromptSnapshot>,
    pub sidebar: WebSidebarSnapshot,
    pub feed_blocks: Vec<crate::ui::feed::WebFeedBlock>,
    pub feed_lines: Vec<String>,
    pub dags: Vec<WebDagRunSnapshot>,
    pub subagents: Vec<WebSubagentJobSnapshot>,
}

impl WebStatus {
    /// Convert a `DagRun` (engine state) into the wire snapshot form.
    pub fn from_dag_run(run: &DagRun) -> WebDagRunSnapshot {
        WebDagRunSnapshot {
            id: run.id.clone(),
            name: run.name.clone(),
            kind: run.kind.as_str().to_string(),
            status: dag_status_str(&run.status).to_string(),
            fail_fast: run.fail_fast,
            max_concurrency: run.max_concurrency,
            direction: match run.direction {
                Direction::Td => "TD".to_string(),
                Direction::Lr => "LR".to_string(),
            },
            created_at: run.created_at,
            completed_at: run.completed_at,
            error: run.error.clone(),
            nodes: run.nodes.iter().map(WebStatus::from_dag_node).collect(),
        }
    }

    fn from_dag_node(node: &DagNode) -> WebDagNodeSnapshot {
        WebDagNodeSnapshot {
            id: node.id.clone(),
            agent: node.agent.clone(),
            status: node_status_str(&node.status).to_string(),
            depends_on: node.depends_on.clone(),
            job_id: node.job_id.clone(),
            attempt: node.attempt,
            started_at: node.started_at,
            completed_at: node.completed_at,
            error: node.error.clone(),
            input_tokens: node.input_tokens,
            output_tokens: node.output_tokens,
            result: node.result.as_ref().map(WebStatus::from_node_result),
            output_tail: node.output.clone(),
            live_preview: node.live_preview.clone(),
        }
    }

    fn from_node_result(result: &NodeResult) -> WebNodeResultSnapshot {
        WebNodeResultSnapshot {
            success: result.success,
            error: result.error.clone(),
            duration_ms: result.duration_ms,
            attempt: result.attempt,
            total_attempts: result.total_attempts,
        }
    }
}

/// Convert one subagent job (registry state) into the wire snapshot form.
pub fn subagent_job_snapshot(
    job: &theway_core::runtime::subagents::registry::SubagentJob,
) -> WebSubagentJobSnapshot {
    WebSubagentJobSnapshot {
        id: job.id.clone(),
        agent: job.agent.clone(),
        source: job.source.clone(),
        run_id: job.run_id.clone(),
        node_id: job.node_id.clone(),
        status: job.status.as_str().to_string(),
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
        output_tail: Some(job.output.clone()),
        live_preview: if job.status == theway_core::runtime::subagents::registry::JobStatus::Running
        {
            Some(job.output.clone())
        } else {
            None
        },
        tps: job.tps(),
        cps: job.cps(),
        chars: Some(job.chars),
        tools_called: Some(job.tools_called),
        turn: Some(job.turn),
    }
}

pub fn node_status_str(status: &NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "pending",
        NodeStatus::Ready => "ready",
        NodeStatus::Running => "running",
        NodeStatus::Succeeded => "succeeded",
        NodeStatus::Failed => "failed",
        NodeStatus::Skipped => "skipped",
        NodeStatus::Cancelled => "cancelled",
    }
}

pub fn dag_status_str(
    status: &theway_core::runtime::graph_engineering::types::DagStatus,
) -> &'static str {
    match status {
        theway_core::runtime::graph_engineering::types::DagStatus::Running => "running",
        theway_core::runtime::graph_engineering::types::DagStatus::Completed => "completed",
        theway_core::runtime::graph_engineering::types::DagStatus::Failed => "failed",
        theway_core::runtime::graph_engineering::types::DagStatus::Cancelled => "cancelled",
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WebGoalSnapshot {
    pub condition: String,
    pub status: String,
    pub iterations: u32,
    pub last_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebControlPlanePromptSnapshot {
    pub tool_name: String,
    pub label: String,
    pub reason: String,
    pub args_hash: String,
    pub payload: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSidebarSnapshot {
    pub inbox_new: usize,
    pub skills: WebSkillsSnapshot,
    pub triggers: WebTriggersSnapshot,
    pub cron: WebCronSnapshot,
    pub mcp: WebMcpSnapshot,
    pub tools: WebToolsSnapshot,
    pub hooks: Vec<String>,
    pub runtime: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSkillsSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub builtin: usize,
    pub user: usize,
    pub project: usize,
    pub items: Vec<WebSkillSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebSkillSnapshot {
    pub name: String,
    pub source: String,
    pub file_path: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebTriggersSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub rules: Vec<WebTriggerRuleSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebTriggerRuleSnapshot {
    pub id: String,
    pub full_id: String,
    pub enabled: bool,
    pub mode: String,
    pub condition: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebCronSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub jobs: Vec<WebCronJobSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebCronJobSnapshot {
    pub id: String,
    pub enabled: bool,
    pub schedule: String,
    pub action: String,
    pub skipped_overlap_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebMcpSnapshot {
    pub servers: usize,
    pub tools: usize,
    pub notification_hooks: usize,
    pub server_names: Vec<String>,
    pub tool_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WebToolsSnapshot {
    pub total: usize,
    pub names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PromptRequest {
    pub text: String,
    #[serde(default)]
    pub images: Vec<WebPromptImage>,
    /// Target session (optional; must be the active session — see
    /// `POST /prompt` validation in theway-server).
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WebPromptImage {
    pub data: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CompleteRequest {
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct CompleteResponse {
    pub completions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CommandAccepted {
    pub accepted: bool,
}

#[derive(Debug, Deserialize)]
pub struct ControlPlaneDecisionRequest {
    pub approve: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetModelRequest {
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct TriggerRuleRequest {
    pub id: String,
}
