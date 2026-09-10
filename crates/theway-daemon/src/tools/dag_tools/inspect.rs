//! `dag_inspect` — single-node detail: status, deps, attempts, error, and the
//! subagent result output (tail-truncated) plus the live preview while running.
//! `kind=transcript` renders the node's registry job transcript as a typed
//! message stream (user / assistant / thinking / tool-call / tool-result)
//! followed by the per-turn summary (token spend, tools per turn, first file
//! write).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::model::node_status_label;
use theway_core::multiagent::graph::types::DagNode;
use theway_core::multiagent::jobs::{
    JobTurnSummary, SubagentJob, SubagentJobRegistry, SubagentJobStatus,
};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::NODE_RESULT_DEFAULT_TAIL;
use super::utils::{node_result_text, ok_text, resolve_dag, tail_truncate};

pub struct DagInspectTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
    pub(super) registry: SubagentJobRegistry,
}

#[async_trait]
impl AgentTool for DagInspectTool {
    fn definition(&self) -> &Tool {
        &INSPECT_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_inspect"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let node_id = params.get("nodeId").and_then(|v| v.as_str());
        let Some(node_id) = node_id else {
            return Ok(ok_text("缺少 nodeId 参数。".to_string()));
        };
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        let kind = params
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("summary");
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let Some(node) = run.node(node_id) else {
                    let ids: Vec<&str> = run.nodes.iter().map(|n| n.id.as_str()).collect();
                    return Ok(ok_text(format!(
                        "{} 中不存在节点 \"{node_id}\"。节点: {}",
                        run.id,
                        ids.join(", ")
                    )));
                };
                let tail = params
                    .get("tail")
                    .and_then(|v| v.as_u64())
                    .filter(|&n| n > 0)
                    .map(|n| n as usize)
                    .unwrap_or(NODE_RESULT_DEFAULT_TAIL);
                let deps = if node.depends_on.is_empty() {
                    "—".to_string()
                } else {
                    node.depends_on
                        .iter()
                        .map(|d| match run.node(d) {
                            Some(dep) => format!("{} ({})", dep.id, node_status_label(&dep.status)),
                            None => format!("{d} (缺失!)"),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let text = if kind == "transcript" {
                    transcript_text(node, &run.id, &self.registry, tail)
                } else {
                    summary_text(node, tail, &deps)
                };
                Ok(ok_text(text))
            }
        }
    }
}

/// Default view: status line, deps, attempts, tokens, output tail, live preview.
fn summary_text(node: &DagNode, tail: usize, deps: &str) -> String {
    let attempt = if node.attempt > 1 {
        format!(" (attempts={})", node.attempt)
    } else {
        String::new()
    };
    format!(
        "{} [{}] — {}{}\n  deps: {}\n{}",
        node.id,
        node.agent,
        node_status_label(&node.status),
        attempt,
        deps,
        node_result_text(node, tail)
    )
}

/// `kind=transcript`: the node's registry job rendered as a typed message
/// stream. Falls back to a note + summary when no job exists (e.g. restored
/// runs whose registry died with the previous process).
fn transcript_text(
    node: &DagNode,
    run_id: &str,
    registry: &SubagentJobRegistry,
    tail: usize,
) -> String {
    let mut parts = vec![format!(
        "{} [{}] — {} · transcript",
        node.id,
        node.agent,
        node_status_label(&node.status)
    )];
    let Some(job) = registry.job_for_node(run_id, &node.id) else {
        parts.push(
            "  无 registry 记录（节点可能来自恢复的运行或旧进程；transcript 只覆盖本进程内启动的节点）"
                .to_string(),
        );
        if let Some(out) = &node.output {
            parts.push(format!(
                "  output (tail {tail}):\n{}",
                tail_truncate(out, tail)
            ));
        }
        return parts.join("\n");
    };
    let mut body = render_transcript(&job);
    if job.status == SubagentJobStatus::Running {
        // In-flight assistant text not yet captured at MessageEnd.
        let live = tail_truncate(&job.output, 2000);
        if !live.is_empty() {
            body.push_str("\n[live text]\n");
            body.push_str(&live);
        }
    }
    if job.messages_truncated {
        parts.push("  (messages 已截断, 仅保留尾部)".to_string());
    }
    parts.push(format!(
        "  messages: {} · status: {:?}",
        job.messages.len(),
        job.status
    ));
    parts.push(tail_truncate(&body, tail));
    // Appended as its own part *after* the truncated body: `tail_truncate` keeps
    // the tail of the transcript, so a summary embedded in `body` would be the
    // first thing a long run drops — exactly the burnt nodes it exists for.
    if let Some(turn_summary) = turn_summary_text(&job) {
        parts.push(turn_summary);
    }
    parts.join("\n")
}

/// Per-turn forensics for `kind=transcript`: turns recorded, tool calls made,
/// the turn that first wrote a file, plus one row per turn and the aggregate
/// tool counts. `None` when the job carries no turn summaries (jobs registered
/// by an older binary, or restored runs whose counter never advanced).
///
/// `first file write` only recognises the `write` and `edit` tools (their
/// labels live in `src/tools/write.rs` / `src/tools/edit.rs`). A turn that
/// creates files through `bash` (redirects, heredocs, `git apply`) is invisible
/// to this scan and reports `none`.
fn turn_summary_text(job: &SubagentJob) -> Option<String> {
    if job.turns.is_empty() {
        return None;
    }
    let tool_calls: usize = job.turns.iter().map(|turn| turn.tools.len()).sum();
    let first_write = job
        .turns
        .iter()
        .filter(|turn| turn.tools.iter().any(|name| is_file_write_tool(name)))
        .map(|turn| turn.index)
        .min();
    let first_write = match first_write {
        Some(index) => format!("turn {index}"),
        None => "none".to_string(),
    };
    let mut out = format!(
        "  per-turn: {} turn(s) · {} tool call(s) · first file write: {first_write}",
        job.turns.len(),
        tool_calls
    );
    for turn in &job.turns {
        let tools = if turn.tools.is_empty() {
            "(no tool call)".to_string()
        } else {
            turn.tools.join(", ")
        };
        out.push_str(&format!(
            "\n    t{:<5}in {:<7}out {:<7}{}",
            turn.index,
            tokens_short(turn.input_tokens),
            tokens_short(turn.output_tokens),
            tools
        ));
    }
    out.push_str(&format!("\n  tools: {}", tool_count_line(&job.turns)));
    if job.turns_truncated {
        out.push_str("\n  (turns 已截断, 最旧的轮次已丢弃)");
    }
    Some(out)
}

/// `true` for the daemon's direct file-writing tools. `bash` can write files
/// too, but a shell command cannot be classified as a write by name alone.
fn is_file_write_tool(name: &str) -> bool {
    name == "write" || name == "edit"
}

/// Aggregate tool counts as `read×6, bash×2`: most-called first, ties broken by
/// name so the line is stable across renders.
fn tool_count_line(turns: &[JobTurnSummary]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for turn in turns {
        for name in &turn.tools {
            *counts.entry(name.as_str()).or_insert(0) += 1;
        }
    }
    let mut rows: Vec<(&str, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    rows.iter()
        .map(|(name, count)| format!("{name}×{count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Token count for display: the plain number below 1000, `x.yk` from 1000 up.
/// Integer arithmetic only — no dependency, no float rounding drift.
fn tokens_short(tokens: u64) -> String {
    if tokens < 1000 {
        return tokens.to_string();
    }
    // Round to the nearest 0.1k.
    let tenths = tokens.saturating_add(50) / 100;
    format!("{}.{}k", tenths / 10, tenths % 10)
}

/// Render registry transcript messages as `[role]`-prefixed sections. LLM
/// messages serialize with the wire `role` discriminator ("user" / "assistant"
/// / "toolResult"); synthetic entries use "toolCall" / "toolResult" plus name /
/// args / content fields. Tolerant: unknown shapes degrade to a one-line dump.
fn render_transcript(job: &SubagentJob) -> String {
    let mut out = String::new();
    for msg in &job.messages {
        let role = msg
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        match role.as_str() {
            "user" => {
                let text = user_content_text(msg.get("content"));
                out.push_str(&format!("[user] {}\n", one_line(&text)));
            }
            "assistant" => {
                let Some(blocks) = msg.get("content").and_then(Value::as_array) else {
                    out.push_str("[assistant] (无内容块)\n");
                    continue;
                };
                let mut text_buf = String::new();
                for block in blocks {
                    let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                    match kind {
                        "text" => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                if !text_buf.is_empty() {
                                    text_buf.push('\n');
                                }
                                text_buf.push_str(t);
                            }
                        }
                        "thinking" => {
                            let thinking =
                                block.get("thinking").and_then(Value::as_str).unwrap_or("");
                            out.push_str(&format!("[thinking] {}\n", cap(thinking, 1200)));
                        }
                        "toolCall" => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
                            let args = block
                                .get("arguments")
                                .map(|a| a.to_string())
                                .unwrap_or_default();
                            out.push_str(&format!("[tool-call] {}({})\n", name, cap(&args, 400)));
                        }
                        "image" => out.push_str("[image]\n"),
                        _ => {}
                    }
                }
                if !text_buf.is_empty() {
                    out.push_str(&format!("[assistant] {}\n", one_line(&text_buf)));
                }
            }
            "toolResult" => {
                // LLM wire shape (content blocks) and the synthetic capture
                // (content string) both render the same way.
                let text = user_content_text(msg.get("content"));
                let name = msg
                    .get("name")
                    .or_else(|| msg.get("toolName"))
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                out.push_str(&format!("[tool-result] {name}: {}\n", cap(&text, 2000)));
            }
            "toolCall" => {
                let name = msg.get("name").and_then(Value::as_str).unwrap_or("?");
                let args = msg.get("args").map(|a| a.to_string()).unwrap_or_default();
                out.push_str(&format!("[tool-call] {}({})\n", name, cap(&args, 400)));
            }
            _ => out.push_str(&format!("[{role}] {}\n", cap(&msg.to_string(), 500))),
        }
    }
    out
}

/// Extract text from a user-content value: plain string, `{"text": …}` object,
/// or an array of text blocks (images ignored).
fn user_content_text(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    match value {
        Value::String(s) => s.clone(),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Value::Array(items) => items
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Collapse a multi-line text into one display line (transcript rows are
/// line-oriented; embedded newlines would break the section framing).
fn one_line(text: &str) -> String {
    text.replace('\n', " ⏎ ")
}

/// Keep the head of `text`, tagging the cut.
fn cap(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_string();
    }
    let head: String = text.chars().take(n).collect();
    format!("{head}…(截断)")
}

static INSPECT_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_inspect".into(),
    description: "Inspect a single DAG node: status, deps, attempts, error, and the subagent result output (tail-truncated) plus the live preview while running. kind=\"transcript\" renders the node's typed message stream (user/assistant/thinking/tool-call/tool-result) plus a per-turn summary (token spend, tools per turn, first file write) instead of the summary.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": { "type": "string", "description": "Node id to inspect (required)" },
            "kind": { "type": "string", "description": "summary (default) or transcript (typed message stream)" },
            "tail": { "type": "number", "description": "Output tail length in chars (default 800)" },
        },
        "required": ["nodeId"],
    }),
}
});

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools/inspect_extra");
