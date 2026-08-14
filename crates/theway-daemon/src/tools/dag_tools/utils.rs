//! Shared helpers for the `dag_*` tools (1:1 port of the dag-orchestrator
//! extension's `tools.ts` helper block). Everything here is module-private
//! (`pub(super)`): consumed by the sibling tool modules and the mirrored test
//! suite in `tests/tools/dag_tools/`.

use std::collections::HashMap;

use serde_json::{Value, json};
use theway_core::AgentToolResult;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::model::{dag_status_label, fmt_dur, node_status_label};
use theway_core::multiagent::graph::types::{DagNode, DagNodeDef, DagRun, DagStatus, NodeStatus};
use theway_llm_provider::UserContentBlock;

pub(super) fn ok_text(content: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![UserContentBlock::text(content)],
        details: json!({}),
        terminate: None,
    }
}

/// `run.sessionId` ownership check: a run belongs to us when the tool has no
/// session id, the run has none, or they match (`!sessionId || !run.sessionId
/// || run.sessionId === sessionId`).
pub(super) fn mine(run: &DagRun, session_id: &Option<String>) -> bool {
    session_id
        .as_deref()
        .is_none_or(|sid| run.session_id.as_deref().is_none_or(|rsid| rsid == sid))
}

pub(super) fn foreign_session(run: &DagRun, session_id: &Option<String>) -> Option<String> {
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

pub(super) fn short8(s: &str) -> String {
    s.chars().take(8).collect()
}

/// TS `resolveDag`: explicit dagId → the run (with the foreign-session guard);
/// omitted → the most recent running run of this session.
pub(super) fn resolve_dag(
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
pub(super) fn node_result_text(node: &DagNode, tail: usize) -> String {
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
pub(super) fn status_counts(run: &DagRun) -> String {
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
pub(super) fn thousands(n: u64) -> String {
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
pub(super) fn tail_truncate(text: &str, tail: usize) -> String {
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
pub(super) fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(super) fn iso_time_ms(ms: i64) -> String {
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
pub(super) fn civil_from_days(z: i64) -> (i64, u32, u32) {
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
pub(super) fn node_def_from_json(n: &Value) -> DagNodeDef {
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
        max_iterations: n
            .get("maxIterations")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        tools: n.get("tools").and_then(|v| v.as_array()).map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        }),
    }
}
