//! `dag_wait` — event-driven harvest: block until DAG(s) reach a terminal
//! state (or the idle watchdog fires), then return every node's result.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::mermaid::run_summary_line;
use theway_core::multiagent::graph::model::dag_status_label;
use theway_core::multiagent::graph::types::{DagRun, DagStatus};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::Tool;
use tokio_util::sync::CancellationToken;

use super::utils::{mine, node_result_text, ok_text, resolve_dag, status_counts};
use super::{DAG_WAIT_DEFAULT_TIMEOUT_SECS, DAG_WAIT_IDLE_SECS, NODE_RESULT_DEFAULT_TAIL};

pub struct DagWaitTool {
    pub(super) engine: Arc<DagEngine>,
    pub(super) session_id: Option<String>,
}

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
        // Parent abort cascades: the wait must not outlive the agent turn. When the
        // parent turn is interrupted, return an informative result instead of a bare
        // error — the DAGs are still running in the background and the agent needs to
        // know that (and how to re-harvest or stop them).
        let results = tokio::select! {
            r = wait => r,
            _ = cancel.cancelled() => {
                let mut lines: Vec<String> = Vec::new();
                for id in &run_ids {
                    if let Some(run) = self.engine.get_run(id) {
                        lines.push(format!("{} ({})", run.id, run_summary_line(&run)));
                    }
                }
                return Ok(ok_text(format!(
                    "dag_wait 被父回合打断 (用户中断/新消息), 提前退出。所等 DAG 仍在后台运行:\n{}\n\n仍可继续: 再调 dag_wait 收割结果; 要终止则 dag_cancel。",
                    lines.join("\n")
                )));
            }
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

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools/wait_extra");
