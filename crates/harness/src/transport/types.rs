//! Wire protocol model shared by the `--web` (axum) and `--grpc` (tonic) transport
//! servers: the command enum both event loops consume and the status payload both
//! serialize. Decoupled from the terminal UI — the servers live in [`super::http`] /
//! [`super::grpc`], the UI stays in `crate::ui`.

use serde::{Deserialize, Serialize};

use theway_core::harness::graph_engineering::types::{
    DagNode, DagRun, Direction, NodeResult, NodeStatus,
};

#[derive(Clone, Debug)]
pub struct WebOptions {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub(crate) enum WebCommand {
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
}

/// graph mode: one DAG run (mirrors `theway_grpc.proto` DagRunSnapshot; task text is
/// deliberately excluded from the wire model — full text goes through GetNodeOutput).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebDagRunSnapshot {
    pub(crate) id: String,
    pub(crate) name: String,
    /// "dag" | "goal" — goal runs are single-node self-loops (condition-terminated).
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) fail_fast: bool,
    pub(crate) max_concurrency: usize,
    pub(crate) direction: String,
    pub(crate) created_at: i64,
    pub(crate) completed_at: Option<i64>,
    pub(crate) error: Option<String>,
    pub(crate) nodes: Vec<WebDagNodeSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebDagNodeSnapshot {
    pub(crate) id: String,
    pub(crate) agent: String,
    pub(crate) status: String,
    pub(crate) depends_on: Vec<String>,
    pub(crate) job_id: Option<String>,
    pub(crate) attempt: u32,
    pub(crate) started_at: Option<i64>,
    pub(crate) completed_at: Option<i64>,
    pub(crate) error: Option<String>,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) result: Option<WebNodeResultSnapshot>,
    pub(crate) output_tail: Option<String>,
    pub(crate) live_preview: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebNodeResultSnapshot {
    pub(crate) success: bool,
    pub(crate) error: Option<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) attempt: u32,
    pub(crate) total_attempts: u32,
}

/// graph mode: one subagent job (mirrors `theway_grpc.proto` SubagentJobSnapshot).
/// Populated from the SubagentJobRegistry in P2; the type ships now so the wire
/// shape is stable.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebSubagentJobSnapshot {
    pub(crate) id: String,
    pub(crate) agent: String,
    pub(crate) source: String,
    pub(crate) run_id: Option<String>,
    pub(crate) node_id: Option<String>,
    pub(crate) status: String,
    pub(crate) started_at: Option<i64>,
    pub(crate) completed_at: Option<i64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) attempt: u32,
    pub(crate) total_attempts: u32,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) error: Option<String>,
    pub(crate) output_tail: Option<String>,
    pub(crate) live_preview: Option<String>,
    pub(crate) tps: Option<f64>,
    pub(crate) cps: Option<f64>,
    pub(crate) chars: Option<u64>,
    pub(crate) tools_called: Option<u64>,
    pub(crate) turn: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebStatus {
    pub(crate) session_id: String,
    pub(crate) model: String,
    pub(crate) model_catalog: Vec<crate::model_picker::ProviderGroup>,
    pub(crate) cwd: String,
    pub(crate) busy: bool,
    pub(crate) queued_count: usize,
    pub(crate) latest_trigger_poll: Option<crate::ui::feed::TriggerPollStatus>,
    pub(crate) goal: Option<WebGoalSnapshot>,
    pub(crate) control_plane_prompt: Option<WebControlPlanePromptSnapshot>,
    pub(crate) sidebar: WebSidebarSnapshot,
    pub(crate) feed_blocks: Vec<crate::ui::feed::WebFeedBlock>,
    pub(crate) feed_lines: Vec<String>,
    pub(crate) dags: Vec<WebDagRunSnapshot>,
    pub(crate) subagents: Vec<WebSubagentJobSnapshot>,
}

impl WebStatus {
    /// Convert a `DagRun` (engine state) into the wire snapshot form.
    pub(crate) fn from_dag_run(run: &DagRun) -> WebDagRunSnapshot {
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

pub(crate) fn node_status_str(status: &NodeStatus) -> &'static str {
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

pub(crate) fn dag_status_str(
    status: &theway_core::harness::graph_engineering::types::DagStatus,
) -> &'static str {
    match status {
        theway_core::harness::graph_engineering::types::DagStatus::Running => "running",
        theway_core::harness::graph_engineering::types::DagStatus::Completed => "completed",
        theway_core::harness::graph_engineering::types::DagStatus::Failed => "failed",
        theway_core::harness::graph_engineering::types::DagStatus::Cancelled => "cancelled",
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebGoalSnapshot {
    pub(crate) condition: String,
    pub(crate) status: String,
    pub(crate) iterations: u32,
    pub(crate) last_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebControlPlanePromptSnapshot {
    pub(crate) tool_name: String,
    pub(crate) label: String,
    pub(crate) reason: String,
    pub(crate) args_hash: String,
    pub(crate) payload: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebSidebarSnapshot {
    pub(crate) inbox_new: usize,
    pub(crate) skills: WebSkillsSnapshot,
    pub(crate) triggers: WebTriggersSnapshot,
    pub(crate) cron: WebCronSnapshot,
    pub(crate) mcp: WebMcpSnapshot,
    pub(crate) tools: WebToolsSnapshot,
    pub(crate) hooks: Vec<String>,
    pub(crate) runtime: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebSkillsSnapshot {
    pub(crate) total: usize,
    pub(crate) enabled: usize,
    pub(crate) disabled: usize,
    pub(crate) builtin: usize,
    pub(crate) user: usize,
    pub(crate) project: usize,
    pub(crate) items: Vec<WebSkillSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebSkillSnapshot {
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) file_path: String,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebTriggersSnapshot {
    pub(crate) total: usize,
    pub(crate) enabled: usize,
    pub(crate) disabled: usize,
    pub(crate) rules: Vec<WebTriggerRuleSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebTriggerRuleSnapshot {
    pub(crate) id: String,
    pub(crate) full_id: String,
    pub(crate) enabled: bool,
    pub(crate) mode: String,
    pub(crate) condition: String,
    pub(crate) action: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebCronSnapshot {
    pub(crate) total: usize,
    pub(crate) enabled: usize,
    pub(crate) disabled: usize,
    pub(crate) jobs: Vec<WebCronJobSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebCronJobSnapshot {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) schedule: String,
    pub(crate) action: String,
    pub(crate) skipped_overlap_count: u64,
    pub(crate) last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebMcpSnapshot {
    pub(crate) servers: usize,
    pub(crate) tools: usize,
    pub(crate) notification_hooks: usize,
    pub(crate) server_names: Vec<String>,
    pub(crate) tool_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct WebToolsSnapshot {
    pub(crate) total: usize,
    pub(crate) names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PromptRequest {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) images: Vec<WebPromptImage>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WebPromptImage {
    pub(crate) data: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CompleteRequest {
    pub(crate) text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CompleteResponse {
    pub(crate) completions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandAccepted {
    pub(crate) accepted: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ControlPlaneDecisionRequest {
    pub(crate) approve: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetModelRequest {
    pub(crate) model: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TriggerRuleRequest {
    pub(crate) id: String,
}
