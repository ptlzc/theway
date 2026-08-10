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
// Layered design:
//   1. preprocess() — dag_plan-subset line scan: classify lines (directive /
//      node / edge / comment / unknown), extract declared ids, and normalize
//      them for mmdr (hyphen ids like `impl-api` are core dag_plan syntax but
//      the vendored parser treats `-` as edge syntax — map them to `_` here
//      and back afterwards). Line-level diagnostics (with line numbers) are
//      collected here, where the original regex parser reported them.
//   2. mermaid_rs_parser (vendored mmdr parse stage) — parses the normalized
//      text: standard mermaid labels/shapes, `&` multi-target, chains, `%%`.
//   3. postprocess — map ids back, split `agent: task` labels, derive
//      depends_on, and cross-check the mmdr node set against the declared
//      ones (mmdr silently mangles unknown lines / stray commas — the check
//      turns that into an explicit error).
//
// Supported subset (documented in the extension README):
//   graph TD|TB|LR (or flowchart)
//   A["agent: task"]          node definition, label = "agent: task"
//   A --> B                   edge
//   A --> B & C               multi-target edges (standard mermaid)
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
/// Chain edge symbols — `A --> B --> C` is split on these before per-segment
/// target parsing (mmdr handles chains natively; preprocess must see the
/// same node set for its declared/consistency bookkeeping).
static EDGE_SYM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-->|-\.->").unwrap());
/// For malformed edge targets (e.g. stray commas) we still register the id
/// prefix so downstream "missing task/agent" diagnostics fire, then error.
static ID_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z0-9_-]+)").unwrap());

pub struct MermaidParseResult {
    pub direction: Direction,
    pub nodes: Vec<DagNodeDef>,
    pub errors: Vec<String>,
}

/// Split a target segment on `&` only when it is outside quotes (labels may
/// legitimately contain `&`, e.g. `A["a: x & y"] --> B`).
fn split_ampersand_outside_quotes(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut in_quote: Option<char> = None;
    let mut start = 0;
    let mut prev = '\0';
    for (i, c) in s.char_indices() {
        match in_quote {
            Some(q) => {
                if c == q && prev != '\\' {
                    in_quote = None;
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    in_quote = Some(c);
                } else if c == '&' {
                    parts.push(&s[start..i]);
                    start = i + 1;
                }
            }
        }
        prev = c;
    }
    parts.push(&s[start..]);
    parts
}

struct Preprocessed {
    /// mmdr-safe text (hyphen ids rewritten to underscores).
    normalized: String,
    /// normalized id → original id (inverse of the rewrite).
    id_map: HashMap<String, String>,
    /// declared node ids (normalized) in declaration order.
    declared: Vec<String>,
    /// ids that were declared WITH an explicit label (mmdr fills label-less
    /// nodes with the id itself; we must not read that back as a task).
    labeled: HashSet<String>,
    errors: Vec<String>,
    direction: Direction,
}

/// Map an original id to its mmdr-safe form, deduplicating collisions.
fn map_id(
    orig: &str,
    id_map: &mut HashMap<String, String>,
    reverse: &mut HashMap<String, String>,
) -> String {
    if let Some(n) = reverse.get(orig) {
        return n.clone();
    }
    let mut n = if orig.contains('-') {
        orig.replace('-', "_")
    } else {
        orig.to_string()
    };
    while id_map.contains_key(&n) {
        n.push('_');
    }
    id_map.insert(n.clone(), orig.to_string());
    reverse.insert(orig.to_string(), n.clone());
    n
}

fn preprocess(text: &str) -> Preprocessed {
    let mut out_lines: Vec<String> = Vec::new();
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut reverse: HashMap<String, String> = HashMap::new();
    let mut declared: Vec<String> = Vec::new();
    let mut labeled: HashSet<String> = HashSet::new();
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
            out_lines.push(line.to_string());
            continue;
        }

        if EDGE_LINE_RE.is_match(line) {
            // Split chains first: `A --> B --> C` → tokens [A, B, C]; each
            // token may itself carry `&` multi-targets.
            let mut tokens: Vec<&str> = Vec::new();
            let mut last = 0usize;
            for m in EDGE_SYM_RE.find_iter(line) {
                tokens.push(line[last..m.start()].trim());
                last = m.end();
            }
            tokens.push(line[last..].trim());
            // Per-token target parsing → (normalized id, label).
            let mut parts: Vec<Vec<(String, Option<String>)>> = Vec::new();
            for token in &tokens {
                let mut ids = Vec::new();
                for seg in split_ampersand_outside_quotes(token) {
                    let seg = seg.trim();
                    if let Some(tm) = TARGET_RE.captures(seg) {
                        let seg_id = tm.get(1).expect("capture 1").as_str();
                        let seg_label = tm.get(2).map(|m| m.as_str());
                        let norm = map_id(seg_id, &mut id_map, &mut reverse);
                        if !declared.contains(&norm) {
                            declared.push(norm.clone());
                        }
                        if seg_label.is_some() {
                            labeled.insert(norm.clone());
                        }
                        ids.push((norm, seg_label.map(|l| l.to_string())));
                    } else {
                        // Malformed target (stray comma etc.): register the id
                        // prefix so node-level diagnostics still fire, then error.
                        if let Some(im) = ID_PREFIX_RE.captures(seg) {
                            let seg_id = im.get(1).expect("capture 1").as_str();
                            let norm = map_id(seg_id, &mut id_map, &mut reverse);
                            if !declared.contains(&norm) {
                                declared.push(norm.clone());
                            }
                            ids.push((norm, None));
                        }
                        errors.push(format!(
                            "第 {line_no} 行: 无法解析目标节点 \"{}\"",
                            seg.trim()
                        ));
                    }
                }
                parts.push(ids);
            }
            // Rebuild as one chain line; `-.->` and `-->` share dag_plan
            // semantics (mmdr keeps the style, we don't consume it).
            let mut rebuilt: Vec<String> = Vec::new();
            for part in &parts {
                let joined: Vec<String> = part
                    .iter()
                    .map(|(n, l)| match l {
                        Some(l) => format!("{n}[{l}]"),
                        None => n.clone(),
                    })
                    .collect();
                if rebuilt.is_empty() {
                    rebuilt.push(joined.join(" & "));
                } else {
                    rebuilt.push(format!(" --> {}", joined.join(" & ")));
                }
            }
            out_lines.push(rebuilt.concat());
            continue;
        }

        if let Some(caps) = NODE_ONLY_RE.captures(line) {
            let id = caps.get(1).expect("capture 1").as_str();
            let label = caps.get(2).map(|m| m.as_str());
            let norm = map_id(id, &mut id_map, &mut reverse);
            if !declared.contains(&norm) {
                declared.push(norm.clone());
            }
            if label.is_some() {
                labeled.insert(norm.clone());
            }
            out_lines.push(match label {
                Some(l) => format!("{norm}[{l}]"),
                None => norm,
            });
            continue;
        }

        let shown: String = line.chars().take(60).collect();
        errors.push(format!(
            "第 {line_no} 行: 无法解析 \"{shown}\" (仅支持 graph TD|LR、A[\"agent: task\"]、--> / -.-> 边)"
        ));
    }

    Preprocessed {
        normalized: out_lines.join("\n"),
        id_map,
        declared,
        labeled,
        errors,
        direction,
    }
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

pub fn parse_mermaid(text: &str) -> MermaidParseResult {
    let prep = preprocess(text);
    let mut errors = prep.errors;

    if !prep.declared.is_empty() {
        let parsed = match mermaid_rs_parser::parse_mermaid(&prep.normalized) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("mermaid 解析失败: {e}"));
                return MermaidParseResult {
                    direction: prep.direction,
                    nodes: Vec::new(),
                    errors,
                };
            }
        };
        let graph = parsed.graph;
        if graph.kind != mermaid_rs_parser::DiagramKind::Flowchart {
            errors.push(format!(
                "仅支持 flowchart (graph/flowchart), 收到 {:?}",
                graph.kind
            ));
        }

        // Cross-check: mmdr must produce exactly the declared node set. It
        // silently mangles unknown lines and stray commas otherwise.
        let parsed_ids: HashSet<&str> = graph.nodes.keys().map(|s| s.as_str()).collect();
        let declared_ids: HashSet<&str> = prep.declared.iter().map(|s| s.as_str()).collect();
        for id in &declared_ids {
            if !parsed_ids.contains(id) {
                errors.push(format!("节点 \"{}\" 未被解析器识别", id));
            }
        }
        for id in &parsed_ids {
            if !declared_ids.contains(id) {
                errors.push(format!("解析出未声明的节点 \"{}\"", id));
            }
        }

        // Assemble nodes in declaration order (mmdr's BTreeMap is sorted, the
        // original parser kept declaration order).
        let mut nodes: Vec<DagNodeDef> = Vec::new();
        for norm in &prep.declared {
            let orig = prep
                .id_map
                .get(norm)
                .cloned()
                .unwrap_or_else(|| norm.clone());
            let label = if prep.labeled.contains(norm) {
                graph
                    .nodes
                    .get(norm)
                    .map(|n| n.label.clone())
                    .unwrap_or_default()
            } else {
                // mmdr fills label-less nodes with the id itself; treat them
                // as unlabeled so the agent/task diagnostics below fire.
                String::new()
            };
            // Malformed label (mmdr swallowed a stray comma): surface it.
            if label.contains('"') || label.contains(']') {
                errors.push(format!("节点 \"{orig}\" 的 label 畸形 (含未闭合引号)"));
            }
            let (agent, task) = split_label(&label);
            nodes.push(DagNodeDef {
                id: orig,
                agent: agent.unwrap_or_default(),
                task: task.unwrap_or_default(),
                depends_on: None,
                timeout: None,
                cwd: None,
                model: None,
                thinking: None,
            });
        }

        // Derive depends_on from edges (mmdr ids are the normalized form).
        let mut deps_of: HashMap<String, Vec<String>> = HashMap::new();
        for e in &graph.edges {
            let from = prep
                .id_map
                .get(&e.from)
                .cloned()
                .unwrap_or_else(|| e.from.clone());
            let to = prep
                .id_map
                .get(&e.to)
                .cloned()
                .unwrap_or_else(|| e.to.clone());
            deps_of.entry(to).or_default().push(from);
        }
        for node in &mut nodes {
            let deps = deps_of.remove(&node.id).unwrap_or_default();
            node.depends_on = if deps.is_empty() { None } else { Some(deps) };
        }

        // Node-level diagnostics (agent/task presence), same as before.
        for n in &nodes {
            if n.agent.is_empty() {
                errors.push(format!(
                    "节点 \"{}\" 的 label 需以 \"agent: task\" 格式 (如 A[\"explorer: 调研代码库\"])",
                    n.id
                ));
            }
            if n.task.is_empty() {
                errors.push(format!("节点 \"{}\" 缺少 task 描述", n.id));
            }
        }

        MermaidParseResult {
            direction: prep.direction,
            nodes,
            errors,
        }
    } else {
        // No declared nodes: comments/blank-only input. Nothing for mmdr to
        // parse; keep the original empty-result behavior (no "missing header").
        MermaidParseResult {
            direction: prep.direction,
            nodes: Vec::new(),
            errors,
        }
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
tests_bridge_macro::tests_bridge!("runtime/multiagent/graph/graph");
