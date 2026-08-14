//! Pure DAG graph logic (no IO): status presentation, validation (cycles,
//! unknown refs), run construction, dependency reconciliation (the
//! "auto-trigger" state derivation), downstream closure; mermaid
//! parse/render lives in [`mermaid`]. 1:1 port of the dag-orchestrator
//! extension's `graph.ts`.

pub mod mermaid;

pub use mermaid::*;

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::types::{
    DagNode, DagNodeDef, DagRun, DagRunDef, DagStatus, Direction, NodeStatus, RunKind,
};

// ── status presentation ──────────────────────────────────────────────────────

/// Plain-text status tags (no emoji — LLM- and terminal-friendly). Shown at
/// the start of every node line and inside mermaid labels.
pub fn status_tag(status: &NodeStatus) -> &'static str {
    match status {
        NodeStatus::Pending => "[wait]",
        NodeStatus::Ready => "[ready]",
        NodeStatus::Running => "[run]",
        NodeStatus::Succeeded => "[done]",
        NodeStatus::Failed => "[fail]",
        NodeStatus::Skipped => "[skip]",
        NodeStatus::Cancelled => "[cancel]",
    }
}

pub fn node_status_label(status: &NodeStatus) -> &'static str {
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

pub fn dag_status_label(status: &DagStatus) -> &'static str {
    match status {
        DagStatus::Running => "running",
        DagStatus::Completed => "completed",
        DagStatus::Failed => "failed",
        DagStatus::Cancelled => "cancelled",
    }
}

/// "[a,c]" dependency prefix for a node line (empty for roots).
pub fn deps_prefix(node: &DagNode) -> String {
    if node.depends_on.is_empty() {
        String::new()
    } else {
        format!("[{}] ", node.depends_on.join(","))
    }
}

pub fn is_terminal(status: &NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Succeeded | NodeStatus::Failed | NodeStatus::Skipped | NodeStatus::Cancelled
    )
}

/// Blocked statuses — a node with a blocked dep can never run.
pub fn is_blocked(status: &NodeStatus) -> bool {
    matches!(status, NodeStatus::Failed | NodeStatus::Cancelled)
}

/// Milliseconds → compact duration ("1.5s", "2m30s", "1h2m"); "–" for junk.
pub fn fmt_dur(ms: i64) -> String {
    if ms < 0 {
        return "–".to_string();
    }
    let s = ms / 1000;
    if s < 60 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let m = s / 60;
        if m < 60 {
            format!("{m}m{}s", s % 60)
        } else {
            format!("{}h{}m", m / 60, m % 60)
        }
    }
}

// ── validation ───────────────────────────────────────────────────────────────

static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").unwrap());

pub fn validate_graph(nodes: &[DagNodeDef], known_agents: Option<&[String]>) -> Vec<String> {
    let mut errors: Vec<String> = Vec::new();
    let mut ids: HashSet<String> = HashSet::new();

    for n in nodes {
        if !ID_RE.is_match(&n.id) {
            errors.push(format!(
                "节点 id \"{}\" 非法: 仅允许字母数字和下划线 (mermaid 兼容)",
                n.id
            ));
            continue;
        }
        if ids.contains(&n.id) {
            errors.push(format!("重复的节点 id \"{}\"", n.id));
        }
        ids.insert(n.id.clone());
        if n.agent.is_empty() {
            errors.push(format!("节点 \"{}\" 缺少 agent", n.id));
        } else if let Some(agents) = known_agents {
            if !agents.is_empty() && !agents.contains(&n.agent) {
                errors.push(format!(
                    "节点 \"{}\" 引用了未知 subagent \"{}\" (可用: {} 或 \"none\")",
                    n.id,
                    n.agent,
                    agents.join(", ")
                ));
            }
        }
        if n.task.trim().is_empty() {
            errors.push(format!("节点 \"{}\" 缺少 task 描述", n.id));
        }
        if let Some(deps) = &n.depends_on {
            for dep in deps {
                if dep == &n.id {
                    errors.push(format!("节点 \"{}\" 不能依赖自己", n.id));
                } else if !ids.contains(dep) && !nodes.iter().any(|x| &x.id == dep) {
                    errors.push(format!("节点 \"{}\" 依赖了不存在的节点 \"{}\"", n.id, dep));
                }
            }
        }
    }

    // Cycle detection (Kahn's algorithm over depends_on). `remaining` keeps
    // declaration order so the cycle message is deterministic.
    let mut remaining: Vec<String> = Vec::new();
    for n in nodes {
        if !remaining.iter().any(|r| r == &n.id) {
            remaining.push(n.id.clone());
        }
    }
    let by_id: HashMap<&str, &DagNodeDef> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = 0;
        while i < remaining.len() {
            let id = &remaining[i];
            let node = by_id
                .get(id.as_str())
                .expect("remaining ids come from nodes");
            let deps_satisfied = node
                .depends_on
                .as_ref()
                .is_none_or(|deps| deps.iter().all(|dep| !remaining.iter().any(|r| r == dep)));
            if deps_satisfied {
                remaining.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    if !remaining.is_empty() {
        errors.push(format!("检测到依赖环: {}", remaining.join(", ")));
    }

    errors
}

// ── run construction ─────────────────────────────────────────────────────────

pub fn build_run(def: &DagRunDef) -> DagRun {
    let now = now_ms();
    let nodes: Vec<DagNode> = def
        .nodes
        .iter()
        .map(|n| DagNode {
            id: n.id.clone(),
            agent: n.agent.clone(),
            task: n.task.clone(),
            depends_on: n.depends_on.clone().unwrap_or_default(),
            timeout: n.timeout,
            cwd: n.cwd.clone(),
            model: n.model.clone(),
            thinking: n.thinking.clone(),
            status: NodeStatus::Pending,
            job_id: None,
            attempt: 0,
            launch_gen: 0,
            started_at: None,
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output: None,
            live_preview: None,
            last_active_at: None,
        })
        .collect();
    DagRun {
        id: String::new(), // assigned by the engine registry
        name: def.name.clone(),
        nodes,
        status: DagStatus::Running,
        kind: RunKind::Dag,
        max_concurrency: def.max_concurrency.unwrap_or(10).max(1),
        fail_fast: def.fail_fast.unwrap_or(false),
        direction: def.direction.clone().unwrap_or(Direction::Td),
        created_at: now,
        session_id: None,
        completed_at: None,
        last_activity_at: now,
        error: None,
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ── dependency semantics ─────────────────────────────────────────────────────

/// Downstream closure: every node reachable from `start_id` along edges.
pub fn downstream_closure(nodes: &[DagNode], start_id: &str) -> Vec<String> {
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in nodes {
        for dep in &n.depends_on {
            dependents.entry(dep.as_str()).or_default().push(&n.id);
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut stack = vec![start_id.to_string()];
    while let Some(id) = stack.pop() {
        if let Some(children) = dependents.get(id.as_str()) {
            for child in children {
                if !seen.iter().any(|s| s == child) {
                    seen.push((*child).to_string());
                    stack.push((*child).to_string());
                }
            }
        }
    }
    seen
}

/// Re-derive non-terminal node states from dependency statuses — the heart of
/// "auto-trigger": after any node reaches a terminal state, this marks
///   - downstream of failed/cancelled → cancelled (blocked forever),
///   - all-deps-succeeded → ready (the scheduler picks it up),
///   - otherwise → pending.
///
/// Idempotent; only touches non-terminal, non-running nodes. The live status
/// map is updated as nodes flip so a cancelled dep cascades within one pass.
pub fn reconcile(run: &mut DagRun) {
    let mut statuses: HashMap<String, NodeStatus> = run
        .nodes
        .iter()
        .map(|n| (n.id.clone(), n.status.clone()))
        .collect();
    for node in &mut run.nodes {
        if is_terminal(&node.status) || node.status == NodeStatus::Running {
            continue;
        }
        // Deps missing from the run are dropped (mirrors the TS filter(Boolean)).
        let deps: Vec<&str> = node
            .depends_on
            .iter()
            .filter(|id| statuses.contains_key(id.as_str()))
            .map(|s| s.as_str())
            .collect();
        let blockers: Vec<&str> = deps
            .iter()
            .copied()
            .filter(|id| is_blocked(statuses.get(*id).expect("deps filtered by presence")))
            .collect();
        if !blockers.is_empty() {
            node.status = NodeStatus::Cancelled;
            node.completed_at = Some(now_ms());
            node.error = Some(format!("blocked by {}", blockers.join(", ")));
            statuses.insert(node.id.clone(), NodeStatus::Cancelled);
            continue;
        }
        let all_done = deps.iter().all(|id| {
            matches!(
                statuses.get(*id),
                Some(NodeStatus::Succeeded) | Some(NodeStatus::Skipped)
            )
        });
        node.status = if all_done {
            NodeStatus::Ready
        } else {
            NodeStatus::Pending
        };
        statuses.insert(node.id.clone(), node.status.clone());
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/model");
