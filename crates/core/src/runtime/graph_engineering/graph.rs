//! Pure DAG graph logic (no IO): mermaid parse/render, validation (cycles,
//! unknown refs), dependency reconciliation (the "auto-trigger" state
//! derivation), downstream closure. 1:1 port of the dag-orchestrator
//! extension's `graph.ts`.

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

// ── mermaid parsing (input: dag_plan's `mermaid` param) ──────────────────────
//
// Supported subset (documented in the extension README):
//   graph TD|LR  (or flowchart)
//   A["agent: task"]          node definition, label = "agent: task"
//   A --> B                   edge
//   A --> B, C                multi-target edges
//   A["agent: task"] --> B    node def + edge in one line
//   A -.-> B                  dotted edge (same semantics)
//   %% comment lines

static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]+$").unwrap());
static DIRECTIVE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(graph|flowchart)\s+(TD|TB|LR)\b").unwrap());
static EDGE_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9_-]+)\s*(?:\[([^\]]*)\])?\s*(?:-->|-\.->)\s*(.+)$").unwrap()
});
static NODE_ONLY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_-]+)\s*(?:\[([^\]]*)\])?\s*$").unwrap());
static TARGET_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([A-Za-z0-9_-]+)\s*(?:\[([^\]]*)\])?\s*$").unwrap());

pub struct MermaidParseResult {
    pub direction: Direction,
    pub nodes: Vec<DagNodeDef>,
    pub errors: Vec<String>,
}

/// Splits a label on the first colon (ASCII or fullwidth `：`); the agent part
/// must be a non-empty run of non-whitespace chars immediately before it,
/// mirroring `/^([^:：\s]+)[:：]\s*([\s\S]*)$/`.
fn split_label(raw: &str) -> (Option<String>, Option<String>) {
    let cleaned = raw.trim();
    let unquoted = {
        let bytes = cleaned.as_bytes();
        let len = cleaned.len();
        if len >= 2
            && ((bytes[0] == b'"' && bytes[len - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[len - 1] == b'\''))
        {
            &cleaned[1..len - 1]
        } else {
            cleaned
        }
    };
    let colon = unquoted
        .char_indices()
        .find(|(_, c)| *c == ':' || *c == '：');
    match colon {
        Some((i, c)) if i > 0 && unquoted[..i].chars().all(|c| !c.is_whitespace()) => {
            let agent = unquoted[..i].to_string();
            // Skip by char width: the fullwidth colon is 3 UTF-8 bytes.
            let task = unquoted[i + c.len_utf8()..].trim().to_string();
            (Some(agent), if task.is_empty() { None } else { Some(task) })
        }
        _ => {
            let task = unquoted.to_string();
            (None, if task.is_empty() { None } else { Some(task) })
        }
    }
}

fn register_node(
    nodes: &mut Vec<DagNodeDef>,
    index: &mut HashMap<String, usize>,
    id: &str,
    label_raw: Option<&str>,
) {
    let idx = match index.get(id) {
        Some(&i) => i,
        None => {
            index.insert(id.to_string(), nodes.len());
            nodes.push(DagNodeDef {
                id: id.to_string(),
                agent: String::new(),
                task: String::new(),
                depends_on: None,
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
            });
            nodes.len() - 1
        }
    };
    if let Some(raw) = label_raw {
        let (agent, task) = split_label(raw);
        let node = &mut nodes[idx];
        if let Some(a) = agent {
            node.agent = a;
        }
        if let Some(t) = task {
            node.task = t;
        }
    }
}

pub fn parse_mermaid(text: &str) -> MermaidParseResult {
    let mut nodes: Vec<DagNodeDef> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut direction = Direction::Td;

    for (i, raw) in text.replace("\r\n", "\n").split('\n').enumerate() {
        let line_no = i + 1;
        let line = match raw.find("%%") {
            Some(p) => raw[..p].trim(),
            None => raw.trim(),
        };
        if line.is_empty() {
            continue;
        }

        if DIRECTIVE_RE.is_match(line) {
            if line
                .split_whitespace()
                .any(|t| t.eq_ignore_ascii_case("LR"))
            {
                direction = Direction::Lr;
            }
            continue;
        }

        if let Some(caps) = EDGE_LINE_RE.captures(line) {
            let from_id = caps.get(1).expect("capture 1").as_str();
            let from_label = caps.get(2).map(|m| m.as_str());
            let rest = caps.get(3).expect("capture 3").as_str();
            register_node(&mut nodes, &mut index, from_id, from_label);
            for target_raw in rest.split(',') {
                let Some(tm) = TARGET_RE.captures(target_raw) else {
                    errors.push(format!(
                        "第 {line_no} 行: 无法解析目标节点 \"{}\"",
                        target_raw.trim()
                    ));
                    continue;
                };
                let to_id = tm.get(1).expect("capture 1").as_str();
                let to_label = tm.get(2).map(|m| m.as_str());
                register_node(&mut nodes, &mut index, to_id, to_label);
                edges.push((from_id.to_string(), to_id.to_string()));
            }
            continue;
        }

        if let Some(caps) = NODE_ONLY_RE.captures(line) {
            let id = caps.get(1).expect("capture 1").as_str();
            let label = caps.get(2).map(|m| m.as_str());
            register_node(&mut nodes, &mut index, id, label);
            continue;
        }

        let shown: String = line.chars().take(60).collect();
        errors.push(format!(
            "第 {line_no} 行: 无法解析 \"{shown}\" (仅支持 graph TD|LR、A[\"agent: task\"]、--> / -.-> 边)"
        ));
    }

    // Assemble: dependents derive from edges; keep declaration order stable.
    let mut ordered: Vec<DagNodeDef> = Vec::new();
    for mut n in nodes {
        let deps: Vec<String> = edges
            .iter()
            .filter(|(_, to)| *to == n.id)
            .map(|(from, _)| from.clone())
            .collect();
        n.depends_on = if deps.is_empty() { None } else { Some(deps) };
        if n.agent.is_empty() {
            errors.push(format!(
                "节点 \"{}\" 的 label 需以 \"agent: task\" 格式 (如 A[\"explorer: 调研代码库\"])",
                n.id
            ));
        }
        if n.task.is_empty() {
            errors.push(format!("节点 \"{}\" 缺少 task 描述", n.id));
        }
        ordered.push(n);
    }
    MermaidParseResult {
        direction,
        nodes: ordered,
        errors,
    }
}

// ── validation ───────────────────────────────────────────────────────────────

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

// ── mermaid rendering (output: widget / /dag / dag_status) ───────────────────

/// Status → (fill, stroke) class colors, mirroring graph.ts CLASS_DEFS.
const CLASS_DEFS: [(NodeStatus, &str, &str); 6] = [
    (NodeStatus::Succeeded, "#e6f4ea", "#34a853"),
    (NodeStatus::Running, "#e8f0fe", "#4285f4"),
    (NodeStatus::Failed, "#fce8e6", "#ea4335"),
    (NodeStatus::Cancelled, "#f1f3f4", "#80868b"),
    (NodeStatus::Ready, "#fef7e0", "#f9ab00"),
    (NodeStatus::Pending, "#ffffff", "#dadce0"),
];

const LABEL_MAX: usize = 40;

static NEWLINE_WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\n\s*").unwrap());

/// Escapes a label for a `"..."` mermaid label: backslashes/quotes escaped,
/// newline runs collapsed to a single space.
pub fn escape_mermaid_label(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    NEWLINE_WS_RE.replace_all(&escaped, " ").into_owned()
}

/// "[tag] [deps] id [agent] first-line-of-task" — the compact node line used
/// inside mermaid labels (task truncated to `LABEL_MAX` chars).
pub fn node_short_label(node: &DagNode) -> String {
    format!(
        "{} {}{} [{}] {}",
        status_tag(&node.status),
        deps_prefix(node),
        node.id,
        node.agent,
        first_line(&node.task, LABEL_MAX)
    )
}

pub fn render_mermaid(run: &DagRun) -> String {
    let mut lines: Vec<String> = vec![format!(
        "graph {}",
        match run.direction {
            Direction::Td => "TD",
            Direction::Lr => "LR",
        }
    )];
    for node in &run.nodes {
        let label = escape_mermaid_label(&node_short_label(node));
        lines.push(format!("  {}[\"{label}\"]", node.id));
        for dep in &node.depends_on {
            lines.push(format!("  {dep} --> {}", node.id));
        }
    }
    let mut used: HashSet<NodeStatus> = HashSet::new();
    for n in &run.nodes {
        used.insert(n.status.clone());
    }
    for (status, fill, stroke) in CLASS_DEFS {
        if used.contains(&status) {
            lines.push(format!(
                "  classDef {} fill:{fill},stroke:{stroke}",
                node_status_label(&status)
            ));
        }
    }
    for node in &run.nodes {
        if node.status != NodeStatus::Pending {
            lines.push(format!(
                "  class {} {}",
                node.id,
                node_status_label(&node.status)
            ));
        }
    }
    lines.join("\n")
}

// ── run / node summaries ─────────────────────────────────────────────────────

/// One-line human-readable node summary: tag + deps prefix + id + agent +
/// task first line, then duration / attempts / error when present.
pub fn node_summary_line(node: &DagNode) -> String {
    let mut parts: Vec<String> = vec![format!(
        "{} {}{} [{}] {}",
        status_tag(&node.status),
        deps_prefix(node),
        node.id,
        node.agent,
        first_line(&node.task, 30)
    )];
    if let (Some(completed), Some(started)) = (node.completed_at, node.started_at) {
        parts.push(format!("({})", fmt_dur(completed - started)));
    }
    if node.attempt > 1 {
        parts.push(format!("attempts={}", node.attempt));
    }
    if let Some(err) = &node.error {
        parts.push(format!("— {}", first_line(err, 60)));
    }
    parts.join(" ")
}

/// Aggregate token usage across all nodes of a run (live + terminal).
pub fn run_token_stats(run: &DagRun) -> (u64, u64) {
    let mut input = 0u64;
    let mut output = 0u64;
    for n in &run.nodes {
        input += n.input_tokens.unwrap_or(0);
        output += n.output_tokens.unwrap_or(0);
    }
    (input, output)
}

/// "12345" → "12,345", matching JS `toLocaleString()` on the extension side.
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

/// One-line run status (always the FIRST line of every DAG view), e.g.
/// "dag-1 [migration] — done 2/5 · run 1 · ready 1 · cancel 1 · fail 0 · ↑12,345 ↓6,789 · 45.6 tok/s".
/// tps = Σ per-node outputTokens / own active seconds (startedAt→completedAt),
/// so queue/idle wall time between nodes does not dilute throughput.
pub fn run_summary_line(run: &DagRun) -> String {
    let mut counts: HashMap<NodeStatus, u32> = HashMap::new();
    for n in &run.nodes {
        *counts.entry(n.status.clone()).or_default() += 1;
    }
    let total = run.nodes.len();
    let done = counts.get(&NodeStatus::Succeeded).copied().unwrap_or(0)
        + counts.get(&NodeStatus::Skipped).copied().unwrap_or(0);
    let seg = |s: NodeStatus, label: &str| -> String {
        match counts.get(&s) {
            Some(&c) if c > 0 => format!(" · {label} {c}"),
            _ => String::new(),
        }
    };
    let tail = if run.status == DagStatus::Running {
        String::new()
    } else {
        format!(" [{}]", dag_status_label(&run.status))
    };
    let (input, output) = run_token_stats(run);
    let token_part = if input + output > 0 {
        format!(" · ↑{} ↓{}", thousands(input), thousands(output))
    } else {
        String::new()
    };
    let now = now_ms();
    let mut tps = 0.0f64;
    for n in &run.nodes {
        let Some(started) = n.started_at else {
            continue;
        };
        let active_ms = n.completed_at.unwrap_or(now) - started;
        if active_ms <= 0 {
            continue;
        }
        if n.output_tokens.unwrap_or(0) == 0 {
            continue;
        }
        tps += n.output_tokens.unwrap_or(0) as f64 / (active_ms as f64 / 1000.0);
    }
    let tps_part = if tps > 0.0 {
        format!(" · {tps:.1} tok/s")
    } else {
        String::new()
    };
    format!(
        "{} [{}] — done {done}/{total}{}{}{}{}{}{}{}",
        run.id,
        run.name,
        seg(NodeStatus::Running, "run"),
        seg(NodeStatus::Ready, "ready"),
        seg(NodeStatus::Cancelled, "cancel"),
        seg(NodeStatus::Failed, "fail"),
        token_part,
        tps_part,
        tail
    )
}

// ── tree view ────────────────────────────────────────────────────────────────

/// Tree-style visualization (box-drawing indentation): each node's depth =
/// longest dependency chain from the roots, so multi-dependency nodes sit
/// under their deepest dep. Far more readable in a terminal than mermaid.
pub fn render_tree(run: &DagRun) -> String {
    // Memoized DFS: node depth = 1 + max(dep depths); roots = 0. The
    // `visiting` set breaks cycles that slipped past validation (the TS
    // original would stack-overflow on them).
    fn compute<'a>(
        id: &'a str,
        run: &'a DagRun,
        depth: &mut HashMap<&'a str, usize>,
        visiting: &mut HashSet<&'a str>,
    ) -> usize {
        if let Some(&d) = depth.get(id) {
            return d;
        }
        if !visiting.insert(id) {
            return 0;
        }
        let mut max_dep: i64 = -1;
        if let Some(node) = run.node(id) {
            for dep in &node.depends_on {
                let dd = if run.node(dep).is_some() {
                    compute(dep, run, depth, visiting) as i64
                } else {
                    -1
                };
                max_dep = max_dep.max(dd);
            }
        }
        visiting.remove(id);
        let d = (1 + max_dep) as usize;
        depth.insert(id, d);
        d
    }

    let mut depth: HashMap<&str, usize> = HashMap::new();
    let mut visiting: HashSet<&str> = HashSet::new();
    for node in &run.nodes {
        compute(node.id.as_str(), run, &mut depth, &mut visiting);
    }

    // Render layer by layer; within a layer keep declaration order.
    let mut by_depth: HashMap<usize, Vec<&str>> = HashMap::new();
    for node in &run.nodes {
        let d = depth.get(node.id.as_str()).copied().unwrap_or(0);
        by_depth.entry(d).or_default().push(node.id.as_str());
    }
    let max_depth = by_depth.keys().copied().max().unwrap_or(0);
    let mut lines: Vec<String> = Vec::new();
    for d in 0..=max_depth {
        if let Some(ids) = by_depth.get(&d) {
            for id in ids {
                let node = run.node(id).expect("ids come from the run");
                let prefix = if d == 0 {
                    String::new()
                } else {
                    "  ".repeat(d)
                };
                lines.push(format!(
                    "{prefix}{} {}{} [{}] {}",
                    status_tag(&node.status),
                    deps_prefix(node),
                    node.id,
                    node.agent,
                    first_line(&node.task, 28)
                ));
            }
        }
    }
    lines.join("\n")
}

/// First line of `text` (trimmed), truncated to `max` chars with an ellipsis.
pub fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().next().map(str::trim).unwrap_or("");
    if line.chars().count() > max {
        let truncated: String = line.chars().take(max).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}
#[cfg(test)]
#[path = "../../../tests/runtime/graph_engineering/graph/mod.rs"]
mod tests;
