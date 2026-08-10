//! `dag_*` tools — the DAG orchestration tool face: define (dag_plan), monitor
//! (dag_status / dag_inspect), harvest (dag_wait), intervene (dag_retry /
//! dag_skip / dag_cancel). 1:1 port of the dag-orchestrator extension's
//! `tools.ts`, driving the engine in
//! `theway_core::runtime::graph_engineering::engine` (which owns the
//! scheduler/state machine; the real subagent launcher lives in
//! `super::node_launcher` and is wired in by p3c-wire).
//!
//! Session isolation: runs are stamped with the owning pi session id, and every
//! tool refuses runs owned by another session (multiple concurrent agents in
//! one project never cross-trigger each other's DAGs). The session id is
//! injected at construction by p3c-wire (`None` = no isolation, e.g. REPL).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::runtime::graph_engineering::graph::{
    dag_status_label, fmt_dur, node_status_label, parse_mermaid, render_mermaid, render_tree,
    run_summary_line,
};
use theway_core::runtime::graph_engineering::types::{
    DagNode, DagNodeDef, DagRun, DagRunDef, DagStatus, Direction, NodeStatus,
};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::subagent_specs::builtin_spec_names;

// ── constants ────────────────────────────────────────────────────────────────

const DAG_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 120;
const DAG_WAIT_IDLE_SECS: u64 = 30;
const NODE_RESULT_DEFAULT_TAIL: usize = 800;

// ── construction ─────────────────────────────────────────────────────────────

/// Build the seven `dag_*` tools, all sharing one engine and the owning pi
/// session id (p3c-wire passes `Some(session_id)` from the harness; `None`
/// disables session isolation).
pub struct DagTools;

impl DagTools {
    /// Returns the tool vec rather than Self by contract — p3c-wire calls this to
    /// build the dag_* tool set for the binary.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(engine: Arc<DagEngine>, session_id: Option<String>) -> Vec<Arc<dyn AgentTool>> {
        vec![
            Arc::new(DagPlanTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagStatusTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagInspectTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagWaitTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagRetryTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagSkipTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagCancelTool { engine, session_id }),
        ]
    }
}

/// Shared engine + session id for one dag_* tool.
macro_rules! dag_tool_struct {
    ($name:ident) => {
        pub struct $name {
            engine: Arc<DagEngine>,
            session_id: Option<String>,
        }
    };
}

dag_tool_struct!(DagPlanTool);
dag_tool_struct!(DagStatusTool);
dag_tool_struct!(DagInspectTool);
dag_tool_struct!(DagWaitTool);
dag_tool_struct!(DagRetryTool);
dag_tool_struct!(DagSkipTool);
dag_tool_struct!(DagCancelTool);

// ── shared helpers (1:1 port of tools.ts) ────────────────────────────────────

fn ok_text(content: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(content)],
        details: json!({}),
        terminate: None,
    }
}

/// `run.sessionId` ownership check: a run belongs to us when the tool has no
/// session id, the run has none, or they match (`!sessionId || !run.sessionId
/// || run.sessionId === sessionId`).
fn mine(run: &DagRun, session_id: &Option<String>) -> bool {
    session_id
        .as_deref()
        .is_none_or(|sid| run.session_id.as_deref().is_none_or(|rsid| rsid == sid))
}

fn foreign_session(run: &DagRun, session_id: &Option<String>) -> Option<String> {
    match (session_id.as_deref(), run.session_id.as_deref()) {
        (Some(sid), Some(rsid)) if rsid != sid => Some(format!(
            "{} 属于其他会话 ({}…), 当前会话是 {}…。多 agent 会话的 DAG 相互隔离, 只可操作本会话创建的 DAG。",
            run.id,
            short8(rsid),
            short8(sid)
        )),
        _ => None,
    }
}

fn short8(s: &str) -> String {
    s.chars().take(8).collect()
}

/// TS `resolveDag`: explicit dagId → the run (with the foreign-session guard);
/// omitted → the most recent running run of this session.
fn resolve_dag(
    engine: &DagEngine,
    session_id: &Option<String>,
    dag_id: Option<&str>,
) -> Result<DagRun, String> {
    if let Some(id) = dag_id {
        let run = engine
            .get_run(id)
            .ok_or_else(|| format!("未知 DAG: {id} (可用: dag_status 查看全部)"))?;
        if let Some(err) = foreign_session(&run, session_id) {
            return Err(err);
        }
        return Ok(run);
    }
    let all = engine.list_runs();
    if let Some(run) = all
        .iter()
        .find(|r| r.status == DagStatus::Running && mine(r, session_id))
    {
        return Ok(run.clone());
    }
    if all.is_empty() {
        return Err("当前没有 DAG。先用 dag_plan 定义一个。".to_string());
    }
    let ref_run = all.iter().find(|r| mine(r, session_id)).unwrap_or(&all[0]);
    Err(format!(
        "没有运行中的 DAG。最近的是 {} ({}, {})。请显式指定 dagId。",
        ref_run.id,
        ref_run.name,
        dag_status_label(&ref_run.status)
    ))
}

/// TS `nodeResultText`: status line + task first line + started/duration/
/// tokens/error + output tail + live preview. Outputs come straight from the
/// node (the launcher writes `DagNode.output` / `live_preview`), so the
/// subagents job registry lookup from TS has no Rust equivalent here.
fn node_result_text(node: &DagNode, tail: usize) -> String {
    let mut parts = vec![format!(
        "{} [{}] — {}",
        node.id,
        node.agent,
        node_status_label(&node.status)
    )];
    parts.push(format!(
        "  task: {}",
        node.task.lines().next().unwrap_or("")
    ));
    if let Some(started) = node.started_at {
        parts.push(format!("  started: {}", iso_time_ms(started)));
    }
    if let Some(completed) = node.completed_at {
        parts.push(format!(
            "  duration: {}",
            fmt_dur(completed - node.started_at.unwrap_or(completed))
        ));
    }
    if node.status == NodeStatus::Running {
        if let Some(active) = node.last_active_at {
            let idle = (current_time_ms() - active).max(0);
            parts.push(format!(
                "  last-active: {} ({}s ago)",
                iso_time_ms(active),
                idle / 1000
            ));
        }
    }
    let tokens = node.input_tokens.unwrap_or(0) + node.output_tokens.unwrap_or(0);
    if tokens > 0 {
        parts.push(format!(
            "  tokens: ↑{} ↓{}",
            thousands(node.input_tokens.unwrap_or(0)),
            thousands(node.output_tokens.unwrap_or(0))
        ));
    }
    if let Some(err) = &node.error {
        parts.push(format!("  error: {err}"));
    }
    if let Some(out) = &node.output {
        parts.push(format!(
            "  output (tail {tail}):\n{}",
            tail_truncate(out, tail)
        ));
    }
    if node.status == NodeStatus::Running {
        if let Some(prev) = &node.live_preview {
            parts.push(format!(
                "  live preview (实时输出, tail {tail}):\n{}",
                tail_truncate(prev, tail)
            ));
        }
    }
    parts.join("\n")
}

/// TS `statusCounts`: "done x/y" (+ skipped) then per-status segments.
fn status_counts(run: &DagRun) -> String {
    let mut counts: HashMap<NodeStatus, u32> = HashMap::new();
    for n in &run.nodes {
        *counts.entry(n.status.clone()).or_default() += 1;
    }
    let seg = |label: &str, s: NodeStatus| -> String {
        match counts.get(&s) {
            Some(&c) if c > 0 => format!(" · {label} {c}"),
            _ => String::new(),
        }
    };
    let done = counts.get(&NodeStatus::Succeeded).copied().unwrap_or(0)
        + counts.get(&NodeStatus::Skipped).copied().unwrap_or(0);
    format!(
        "done {done}/{}{}{}{}{}",
        run.nodes.len(),
        seg("run", NodeStatus::Running),
        seg("ready", NodeStatus::Ready),
        seg("cancel", NodeStatus::Cancelled),
        seg("fail", NodeStatus::Failed)
    )
}

/// JS `toLocaleString()` thousands separators ("12345" → "12,345").
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let lead = s.len() % 3;
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && i % 3 == lead {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// TS `out.length > tail ? "…(N 字符, 截断)\n" + out.slice(-tail) : out`.
fn tail_truncate(text: &str, tail: usize) -> String {
    let len = text.chars().count();
    if len <= tail {
        text.to_string()
    } else {
        let last: String = text.chars().skip(len - tail).collect();
        format!("…({len} 字符, 截断)\n{last}")
    }
}

/// Epoch ms → `toISOString()`-style UTC string ("YYYY-MM-DDTHH:MM:SS.mmmZ").
/// Written by hand because chrono's RFC3339 formatters need the `alloc`
/// feature, which this crate's chrono dependency does not enable.
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn iso_time_ms(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let h = ms_of_day / 3_600_000;
    let m = (ms_of_day % 3_600_000) / 60_000;
    let s = (ms_of_day % 60_000) / 1000;
    let milli = ms_of_day % 1000;
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{milli:03}Z")
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01 → (y, m, d)).
/// `div_euclid` replaces the original era branch and stays exact for all i64.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// nodes[] JSON → DagNodeDef (TS coerces non-strings via `String(...)`; missing
/// values become empty strings and get caught by graph validation).
fn node_def_from_json(n: &Value) -> DagNodeDef {
    DagNodeDef {
        id: n
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        agent: n
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        task: n
            .get("task")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        depends_on: n.get("dependsOn").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        }),
        timeout: n.get("timeout").and_then(|v| v.as_u64()),
        cwd: n.get("cwd").and_then(|v| v.as_str()).map(String::from),
        model: n.get("model").and_then(|v| v.as_str()).map(String::from),
        thinking: n.get("thinking").and_then(|v| v.as_str()).map(String::from),
    }
}

// ── dag_plan ─────────────────────────────────────────────────────────────────

/// Build a `DagRunDef` from a name + definition string (mermaid text or JSON
/// nodes[]), mirroring the `dag_plan` tool's parameter handling. Shared with the
/// gRPC `DagPlan` RPC so the wire surface and the tool accept the same input.
pub fn plan_from_definition(
    name: &str,
    definition: &str,
    fail_fast: Option<bool>,
    max_concurrency: Option<usize>,
    direction: Option<Direction>,
) -> Result<DagRunDef, String> {
    let trimmed = definition.trim();
    let (def_nodes, mermaid_dir) = if trimmed.starts_with("graph ") {
        let parsed = parse_mermaid(trimmed);
        if !parsed.errors.is_empty() {
            return Err(format!("mermaid 解析失败:\n{}", parsed.errors.join("\n")));
        }
        (parsed.nodes, Some(parsed.direction))
    } else {
        let arr: Vec<serde_json::Value> =
            serde_json::from_str(trimmed).map_err(|e| format!("definition 不是合法 JSON: {e}"))?;
        (arr.iter().map(node_def_from_json).collect(), None)
    };
    Ok(DagRunDef {
        name: name.to_string(),
        nodes: def_nodes,
        max_concurrency,
        fail_fast,
        direction: direction.or(mermaid_dir),
    })
}

impl DagPlanTool {
    async fn execute_impl(&self, params: Value) -> Result<AgentToolResult, AgentToolError> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(name) = name else {
            return Ok(ok_text("缺少 name (运行标签)。".to_string()));
        };
        let has_nodes = params
            .get("nodes")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty());
        let has_mermaid = params
            .get("mermaid")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if has_nodes && has_mermaid {
            return Ok(ok_text("nodes 和 mermaid 只能提供其一。".to_string()));
        }
        let definition = if has_mermaid {
            params
                .get("mermaid")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        } else if has_nodes {
            params.get("nodes").cloned().unwrap_or_default().to_string()
        } else {
            return Ok(ok_text("需要 nodes[] 或 mermaid 参数。".to_string()));
        };
        let def = match plan_from_definition(
            name,
            &definition,
            params.get("failFast").and_then(|v| v.as_bool()),
            params
                .get("maxConcurrency")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
            if params.get("direction").and_then(|v| v.as_str()) == Some("LR") {
                Some(Direction::Lr)
            } else {
                None
            },
        ) {
            Ok(def) => def,
            Err(e) => return Ok(ok_text(e)),
        };

        let known: Vec<String> = builtin_spec_names().into_iter().map(String::from).collect();
        match self.engine.plan(def, Some(&known), self.session_id.clone()) {
            Err(errors) => Ok(ok_text(format!("DAG 校验失败:\n{}", errors.join("\n")))),
            Ok(run) => {
                let run_id = run.id.clone();
                let text = format!(
                    "✓ 已创建并自动启动 {} [{}] ({} 节点, 并发 {})\n\n{}\n\n{}\n\n监控: dag_status(dagId) 或查看上方 widget; 收割结果: dag_wait(dagId)。失败时用 dag_inspect 看详情, dag_retry/dag_skip 干预。",
                    run.id,
                    run.name,
                    run.nodes.len(),
                    run.max_concurrency,
                    render_mermaid(&run),
                    run_summary_line(&run)
                );
                Ok(AgentToolResult {
                    content: vec![UserContentBlock::text(text)],
                    details: json!({ "runId": run_id }),
                    terminate: None,
                })
            }
        }
    }
}

#[async_trait]
impl AgentTool for DagPlanTool {
    fn definition(&self) -> &Tool {
        &PLAN_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_plan"
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
        self.execute_impl(params).await
    }
}

static PLAN_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_plan".into(),
    description: "Define a DAG of subagent tasks (JSON nodes[] or mermaid flowchart text) and AUTO-START it. The engine launches nodes whose prerequisites all succeeded — you do NOT launch them manually with subagent(background:true). Parameters: name (required label), nodes (array of {id, agent, task, dependsOn?, timeout?, cwd?, model?, thinking?}) OR mermaid (graph TD text; node labels must be \"agent: task\"). Optional: maxConcurrency (default 10), failFast (default false: only the failed node's downstream is cancelled, side branches continue; true: any failure aborts the whole run), direction TD|LR. Session-scoped: runs bind to the calling session — multiple concurrent agents in one project each manage their own DAGs, and dag_* tools refuse runs owned by another session. Node ID convention: short semantic names (lowercase + hyphen), e.g. explore → plan → impl → verify; parallel siblings get suffixes (impl-api, impl-web); a numeric phase prefix is allowed (1-explore, 2-plan). Every non-root node MUST declare dependsOn — the status display shows \"[deps] id\" so prerequisites are always visible. One session can own MULTIPLE DAGs in parallel (dag-1, dag-2, …): dag_wait with no dagId harvests ALL running DAGs of the session; comma-separated dagIds wait for a subset. Returns the run id + rendered mermaid status graph. Monitor via dag_status / the live widget; harvest via dag_wait.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "description": "Run label, e.g. \"migration\" (required)" },
            "nodes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Unique node id (letters/digits/underscore)" },
                        "agent": { "type": "string", "description": "Subagent name from the spec registry" },
                        "task": { "type": "string", "description": "Task prompt for the subagent" },
                        "dependsOn": { "type": "array", "items": { "type": "string" }, "description": "Prerequisite node ids" },
                        "timeout": { "type": "number", "description": "Idle timeout override (sec)" },
                        "cwd": { "type": "string", "description": "Working directory (absolute path) for the subagent; pinned into its system prompt. Required for multi-repo tasks — without it the subagent operates in the session cwd" },
                        "model": { "type": "string", "description": "Primary-target model override" },
                        "thinking": { "type": "string", "description": "Primary-target thinking override" },
                    },
                    "required": ["id", "agent", "task"],
                },
                "description": "Node definitions (JSON form; mutually exclusive with mermaid)",
            },
            "mermaid": {
                "type": "string",
                "description": "Mermaid flowchart text, e.g. graph TD\\n  A[\"explorer: 调研\"] --> B[\"planner: 计划\"]",
            },
            "maxConcurrency": { "type": "number", "description": "Max concurrently running nodes (default 10)" },
            "failFast": { "type": "boolean", "description": "true: any failure aborts everything. false (default): only the failed node's downstream is cancelled" },
            "direction": { "type": "string", "enum": ["TD", "LR"], "description": "Mermaid direction (default TD)" },
        },
        "required": ["name"],
    }),
}
});

// ── dag_status ───────────────────────────────────────────────────────────────

#[async_trait]
impl AgentTool for DagStatusTool {
    fn definition(&self) -> &Tool {
        &STATUS_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_status"
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
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        if dag_id.is_none() {
            let runs = self.engine.list_runs();
            if runs.is_empty() {
                return Ok(ok_text("当前没有 DAG。用 dag_plan 定义一个。".to_string()));
            }
            let lines: Vec<String> = runs
                .iter()
                .map(|r| format!("{}\n{}", run_summary_line(r), render_tree(r)))
                .collect();
            return Ok(ok_text(format!(
                "共 {} 个 DAG:\n\n{}",
                runs.len(),
                lines.join("\n\n")
            )));
        }
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let suffix = run
                    .error
                    .as_deref()
                    .map(|e| format!(" — {e}"))
                    .unwrap_or_default();
                Ok(ok_text(format!(
                    "{}{}\n\n依赖树:\n{}\n\nmermaid (可粘贴到 mermaid.live):\n{}",
                    run_summary_line(&run),
                    suffix,
                    render_tree(&run),
                    render_mermaid(&run)
                )))
            }
        }
    }
}

static STATUS_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_status".into(),
    description: "Show DAG state: the run summary, the status-styled mermaid graph, and a per-node table. No dagId: lists all runs. With dagId: full detail for that run. Mermaid text can be pasted into mermaid.live to visualize.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (e.g. dag-1). Omit to list all runs." },
        },
    }),
}
});

// ── dag_inspect ──────────────────────────────────────────────────────────────

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
                let attempt = if node.attempt > 1 {
                    format!(" (attempts={})", node.attempt)
                } else {
                    String::new()
                };
                Ok(ok_text(format!(
                    "{} [{}] — {}{}\n  deps: {}\n{}",
                    node.id,
                    node.agent,
                    node_status_label(&node.status),
                    attempt,
                    deps,
                    node_result_text(node, tail)
                )))
            }
        }
    }
}

static INSPECT_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_inspect".into(),
    description: "Inspect a single DAG node: status, deps, attempts, error, and the subagent result output (tail-truncated). Use when a node failed or you need the actual output of a succeeded node.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": { "type": "string", "description": "Node id to inspect (required)" },
            "tail": { "type": "number", "description": "Output tail length in chars (default 800)" },
        },
        "required": ["nodeId"],
    }),
}
});

// ── dag_wait ─────────────────────────────────────────────────────────────────

#[async_trait]
impl AgentTool for DagWaitTool {
    fn definition(&self) -> &Tool {
        &WAIT_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_wait"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let raw = params.get("dagId").and_then(|v| v.as_str());
        let run_ids: Vec<String> = if let Some(raw) = raw {
            let mut ids = Vec::new();
            for part in raw.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                match resolve_dag(&self.engine, &self.session_id, Some(part)) {
                    Ok(run) => ids.push(run.id),
                    Err(msg) => return Ok(ok_text(msg)),
                }
            }
            ids
        } else {
            let all = self.engine.list_runs();
            let running: Vec<&DagRun> = all
                .iter()
                .filter(|r| r.status == DagStatus::Running && mine(r, &self.session_id))
                .collect();
            if running.is_empty() {
                let msg = if all.is_empty() {
                    "当前没有 DAG。先用 dag_plan 定义一个。".to_string()
                } else {
                    let ref_run = all
                        .iter()
                        .find(|r| mine(r, &self.session_id))
                        .unwrap_or(&all[0]);
                    format!(
                        "本会话没有运行中的 DAG。最近的是 {} ({}, {})。请显式指定 dagId。",
                        ref_run.id,
                        ref_run.name,
                        dag_status_label(&ref_run.status)
                    )
                };
                return Ok(ok_text(msg));
            }
            running.iter().map(|r| r.id.clone()).collect()
        };

        let timeout_secs = params
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DAG_WAIT_DEFAULT_TIMEOUT_SECS);
        let wait = self.engine.wait_for_runs(
            &run_ids,
            Duration::from_secs(timeout_secs),
            Some(Duration::from_secs(DAG_WAIT_IDLE_SECS)),
        );
        // Parent abort cascades: the wait must not outlive the agent turn.
        let results = tokio::select! {
            r = wait => r,
            _ = cancel.cancelled() => return Err(AgentToolError::Message("cancelled".into())),
        };
        let mut runs: Vec<(DagRun, bool)> = Vec::new();
        for (id, timed_out) in results {
            if let Some(run) = self.engine.get_run(&id) {
                runs.push((run, timed_out));
            }
        }
        let any_timed_out = runs.iter().any(|(_, t)| *t);
        let sections: Vec<String> = runs
            .iter()
            .map(|(run, timed_out)| {
                let parts: Vec<String> = run
                    .nodes
                    .iter()
                    .map(|n| node_result_text(n, NODE_RESULT_DEFAULT_TAIL))
                    .collect();
                let head = if *timed_out {
                    format!(
                        "{} 尚未结束 ({}s 超时或无活动)。当前状态:\n{}",
                        run.id,
                        timeout_secs,
                        run_summary_line(run)
                    )
                } else {
                    let st = match run.status {
                        DagStatus::Completed => "完成",
                        DagStatus::Cancelled => "取消",
                        _ => "结束 (存在失败)",
                    };
                    format!("{} 已{}: {}", run.id, st, status_counts(run))
                };
                let mut section = format!("{head}\n{}", parts.join("\n\n"));
                if run.status == DagStatus::Failed {
                    section.push_str(
                        "\n\n失败处理: dag_inspect 看错误 → dag_retry 重跑 (会自动重放受影响子图) 或 dag_skip 跳过。",
                    );
                }
                section
            })
            .collect();
        let head = if any_timed_out {
            let ids: Vec<&str> = runs
                .iter()
                .filter(|(_, t)| *t)
                .map(|(r, _)| r.id.as_str())
                .collect();
            format!(
                "共 {} 个 DAG, 尚未全部结束 ({}s 超时或无活动): {} 仍在运行。\n\n仍可继续: dag_wait 再等 / dag_retry / dag_skip / dag_cancel。\n\n",
                runs.len(),
                timeout_secs,
                ids.join(", ")
            )
        } else {
            let ids: Vec<String> = runs
                .iter()
                .map(|(r, _)| format!("{} ({})", r.id, dag_status_label(&r.status)))
                .collect();
            format!(
                "共 {} 个 DAG 收割完毕: {}。\n\n",
                runs.len(),
                ids.join(", ")
            )
        };
        Ok(ok_text(format!("{head}{}", sections.join("\n\n---\n\n"))))
    }
}

static WAIT_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_wait".into(),
    description: "Block until DAG(s) reach a terminal state, then return every node's result (status, error, output tail). Event-driven, no polling. Completions are queued — calling again returns immediately if the run already finished. dagId: ONE run id, several comma-separated (\"dag-1,dag-2\"), or OMIT to wait for ALL running DAGs of the current session (multi-DAG orchestration). Parameters: dagId (default: all running DAGs of this session), timeout (sec, default 120). Idle watchdog: returns early after 30s without any node activity so you can inspect status and decide (wait again / retry / skip / cancel).".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id, comma-separated ids, or omit to wait for all running DAGs of this session (default)" },
            "timeout": { "type": "number", "description": "Max wait seconds (default 120)" },
        },
    }),
}
});

// ── dag_retry ────────────────────────────────────────────────────────────────

#[async_trait]
impl AgentTool for DagRetryTool {
    fn definition(&self) -> &Tool {
        &RETRY_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_retry"
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
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let node_id = params.get("nodeId").and_then(|v| v.as_str());
                let ids: Option<Vec<String>> = match node_id {
                    Some(id) if id != "failed" => Some(vec![id.to_string()]),
                    _ => None,
                };
                let run_id = run.id.clone();
                let reset = self.engine.retry(&run_id, ids.as_deref());
                let run = self.engine.get_run(&run_id).unwrap_or(run);
                if reset.is_empty() {
                    return Ok(ok_text(format!(
                        "{} 没有可重试的节点 (仅 failed/cancelled 节点可重试)。\n{}",
                        run.id,
                        run_summary_line(&run)
                    )));
                }
                Ok(ok_text(format!(
                    "✓ 已重置 {} 个节点: {}\n\n{}\n\n{}",
                    reset.len(),
                    reset.join(", "),
                    render_tree(&run),
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static RETRY_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_retry".into(),
    description: "Re-run blocked nodes of a DAG. Without nodeId: all failed+cancelled nodes. With nodeId: that node plus its blocked downstream closure. Also restarts a terminal run (e.g. after dag_cancel or a completed run you want to re-execute). Returns the reset node ids and the new graph state.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": {
                "type": "string",
                "description": "Node id to retry, or \"failed\" (all failed+cancelled nodes, default). Omit to reset all blocked nodes.",
            },
        },
    }),
}
});

// ── dag_skip ─────────────────────────────────────────────────────────────────

#[async_trait]
impl AgentTool for DagSkipTool {
    fn definition(&self) -> &Tool {
        &SKIP_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_skip"
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
        let node_id = params
            .get("nodeId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let Some(node_id) = node_id else {
            return Ok(ok_text("缺少 nodeId 参数。".to_string()));
        };
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                let run_id = run.id.clone();
                if !self.engine.skip(&run_id, &node_id) {
                    let why = match run.node(&node_id) {
                        Some(n) => format!("节点已是 {}", node_status_label(&n.status)),
                        None => "节点不存在".to_string(),
                    };
                    return Ok(ok_text(format!("无法跳过 \"{node_id}\": {why}。")));
                }
                let run = self.engine.get_run(&run_id).unwrap_or(run);
                Ok(ok_text(format!(
                    "✓ 已跳过 {node_id} (下游将视为成功继续)。\n\n{}\n\n{}",
                    render_tree(&run),
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static SKIP_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_skip".into(),
    description: "Skip a node: mark it as skipped (counts as success for downstream, so dependents become ready). If the node is currently running, its job is aborted first. Use when a node is unnecessary or its work was already done elsewhere.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
            "nodeId": { "type": "string", "description": "Node id to skip (required)" },
        },
        "required": ["nodeId"],
    }),
}
});

// ── dag_cancel ───────────────────────────────────────────────────────────────

#[async_trait]
impl AgentTool for DagCancelTool {
    fn definition(&self) -> &Tool {
        &CANCEL_DEFINITION
    }

    fn label(&self) -> &str {
        "dag_cancel"
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
        let dag_id = params.get("dagId").and_then(|v| v.as_str());
        match resolve_dag(&self.engine, &self.session_id, dag_id) {
            Err(msg) => Ok(ok_text(msg)),
            Ok(run) => {
                if run.status != DagStatus::Running {
                    return Ok(ok_text(format!(
                        "{} 已处于终态 ({}), 无需取消。",
                        run.id,
                        dag_status_label(&run.status)
                    )));
                }
                let run_id = run.id.clone();
                let run_name = run.name.clone();
                self.engine.cancel_run(&run_id, None);
                let run = self.engine.get_run(&run_id).expect("run exists");
                Ok(ok_text(format!(
                    "✓ 已取消 {run_id} [{run_name}]: 运行中的任务已终止, 其余节点标记 cancelled。\n\n{}\n\n重新执行: dag_retry(dagId) 会重置全部 blocked 节点并重启。",
                    run_summary_line(&run)
                )))
            }
        }
    }
}

static CANCEL_DEFINITION: Lazy<Tool> = Lazy::new(|| {
    Tool {
    name: "dag_cancel".into(),
    description: "Cancel a DAG: aborts all running node jobs and marks pending/ready nodes as cancelled. Use dag_retry afterwards to re-run the whole thing, or dag_skip to salvage parts.".into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "dagId": { "type": "string", "description": "Run id (default: most recent active run)" },
        },
    }),
}
});

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
tests_bridge!("../../tests/tools/dag_tools/mod.rs");
