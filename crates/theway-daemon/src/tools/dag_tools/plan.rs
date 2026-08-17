//! `dag_plan` — define a DAG of subagent tasks (JSON nodes[] or mermaid) and
//! auto-start it; the engine launches nodes whose prerequisites all succeeded.

use std::sync::Arc;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::model::{parse_mermaid, render_mermaid, run_summary_line};
use theway_core::multiagent::graph::types::{DagRunDef, Direction};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::utils::{node_def_from_json, ok_text};

/// `dag_plan` additionally carries the known spec names (app-side table) so the
/// agent field is validated against the real spec registry.
pub struct DagPlanTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
    pub(super) spec_names: Vec<String>,
}

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

        match self
            .engine
            .plan(def, Some(&self.spec_names), self.session_id.clone())
        {
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
    description: "Define a DAG of subagent tasks (JSON nodes[] or mermaid flowchart text) and AUTO-START it. The engine launches nodes whose prerequisites all succeeded — you do NOT launch them manually with subagent(background:true). Parameters: name (required label), nodes (array of {id, agent, task, dependsOn?, timeout?, cwd?, model?, thinking?, maxIterations?, tools?}) OR mermaid (graph TD text; node labels must be \"agent: task\"). Optional: maxConcurrency (default 10), failFast (default false: only the failed node's downstream is cancelled, side branches continue; true: any failure aborts the whole run), direction TD|LR. Node budgets: every node's subagent defaults to 300 LLM-turn attempts (code-harness budget — compile/fix loops need it); for short, fast tasks (a quick read, a single check) set maxIterations to a smaller range like 4-32. Node tools: by default a node's subagent gets the orchestrator tool set minus dag_* and subagent; to restrict a node to specific tools set tools: [\"read\", \"bash\"] (unknown tool names fail the node). Session-scoped: runs bind to the calling session — multiple concurrent agents in one project each manage their own DAGs, and dag_* tools refuse runs owned by another session. Node ID convention: short semantic names (lowercase + hyphen), e.g. explore → plan → impl → verify; parallel siblings get suffixes (impl-api, impl-web); a numeric phase prefix is allowed (1-explore, 2-plan). Every non-root node MUST declare dependsOn — the status display shows \"[deps] id\" so prerequisites are always visible. One session can own MULTIPLE DAGs in parallel (dag-1, dag-2, …): dag_wait with no dagId harvests ALL running DAGs of the session; comma-separated dagIds wait for a subset. Returns the run id + rendered mermaid status graph. Monitor via dag_status / the live widget; harvest via dag_wait.".into(),
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
                        "maxIterations": { "type": "number", "description": "Iteration-budget override (LLM-turn attempts) for this node's subagent; defaults to 300 (code-harness budget), lower to 4-32 for short, fast tasks" },
                        "tools": { "type": "array", "items": { "type": "string" }, "description": "Tool allowlist (tool names) for this node's subagent; omitted means the full resolved tool set, unknown tool names fail the node" },
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

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools/plan_extra");
