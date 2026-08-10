//! DAG orchestration engine: the scheduler/state machine. Registers runs,
//! auto-triggers nodes whose prerequisites all succeeded (event-driven via
//! launcher callbacks), enforces the concurrency budget, and exposes
//! retry/skip/cancel/wait. 1:1 port of the dag-orchestrator extension's
//! `engine.ts`; execution is delegated to a `NodeLauncher` (implemented by
//! the coding-agent side — the engine only schedules).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::graph::{
    build_run, downstream_closure, is_blocked, is_terminal, now_ms, validate_graph,
};
use super::persist::{PersistedRun, hydrate, max_run_counter};
use super::types::{
    DagEvent, DagNode, DagRun, DagRunDef, DagStatus, Direction, NodeResult, NodeStatus, RunKind,
};

/// Node execution result reported by the launcher when the subagent job ends.
#[derive(Clone, Debug)]
pub struct NodeOutcome {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub attempt: u32,
    pub total_attempts: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub output: Option<String>,
}

/// Execution abstraction the engine drives. The launcher spawns its own task
/// and reports completion via [`DagEngine::on_node_completed`]; `cancel` is
/// aborted on run cancel / skip.
pub trait NodeLauncher: Send + Sync {
    fn launch(&self, run_id: &str, node_id: &str, cancel: CancellationToken);
}

/// Shared scheduler handle (cheap clone, same as the TS module singleton).
pub struct DagEngine {
    inner: Arc<Mutex<EngineInner>>,
}

struct EngineInner {
    runs: HashMap<String, DagRun>,
    dag_counter: u64,
    /// Independent id sequence for goal runs (goal-N, self-loop semantics).
    goal_counter: u64,
    launcher: Option<Arc<dyn NodeLauncher>>,
    /// Event-plane broadcast (node_status / run_status). `None` = detached.
    events: Option<tokio::sync::broadcast::Sender<DagEvent>>,
    /// Abort tokens for in-flight nodes, keyed by (run_id, node_id).
    jobs: HashMap<(String, String), CancellationToken>,
    /// Condition variable per run for `wait_for_runs` (created on demand).
    waiters: HashMap<String, Arc<Notify>>,
}

impl Clone for DagEngine {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl std::fmt::Debug for DagEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.lock();
        f.debug_struct("DagEngine")
            .field("runs", &inner.runs.len())
            .field("dag_counter", &inner.dag_counter)
            .field("goal_counter", &inner.goal_counter)
            .field("launcher", &inner.launcher.is_some())
            .field("events", &inner.events.is_some())
            .field("running_jobs", &inner.jobs.len())
            .field("waiters", &inner.waiters.len())
            .finish()
    }
}

impl DagEngine {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                runs: HashMap::new(),
                dag_counter: 0,
                goal_counter: 0,
                launcher: None,
                events: None,
                jobs: HashMap::new(),
                waiters: HashMap::new(),
            })),
        }
    }

    /// Override the node launcher (tests inject a fake one).
    pub fn set_launcher(&self, launcher: Option<Arc<dyn NodeLauncher>>) {
        self.inner.lock().launcher = launcher;
    }

    /// Wire the event-plane broadcast (transport setup calls this once);
    /// `None` detaches (same contract as SubagentJobRegistry).
    pub fn set_event_sender(&self, tx: Option<tokio::sync::broadcast::Sender<DagEvent>>) {
        self.inner.lock().events = tx;
    }

    /// Broadcast an event-plane message (no-op without a sender).
    fn emit(&self, event: DagEvent) {
        if let Some(tx) = self.inner.lock().events.clone() {
            let _ = tx.send(event);
        }
    }

    // ── registry ────────────────────────────────────────────────────────────

    /// Validate + register + auto-start a DAG. Errors = graph validation
    /// errors (Chinese copy lives in graph.rs).
    pub fn plan(
        &self,
        def: DagRunDef,
        known_agents: Option<&[String]>,
        session_id: Option<String>,
    ) -> Result<DagRun, Vec<String>> {
        let errors = validate_graph(&def.nodes, known_agents);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut run = build_run(&def);
        run.session_id = session_id;
        {
            let mut inner = self.inner.lock();
            inner.dag_counter += 1;
            run.id = format!("dag-{}", inner.dag_counter);
            run.last_activity_at = now_ms();
            inner.runs.insert(run.id.clone(), run.clone());
        }
        self.reconcile(&run.id);
        self.tick(&run.id);
        Ok(self.get_run(&run.id).unwrap_or(run))
    }

    // ── goal runs (single-node self-loops, driven by the goal.rs hook) ──────

    /// Register a goal run: one `main` node (agent main-agent) that loops
    /// until the condition terminates it via `on_goal_tick`/`complete_goal`.
    /// Returns the run id (`goal-N`, independent counter from dag-N).
    pub fn plan_goal(&self, condition: &str, session_id: Option<String>) -> String {
        let now = now_ms();
        let session_id_str = session_id.clone().unwrap_or_default();
        let node = DagNode {
            id: "main".to_string(),
            agent: "main-agent".to_string(),
            task: condition.to_string(),
            depends_on: Vec::new(),
            timeout: None,
            cwd: None,
            model: None,
            thinking: None,
            status: NodeStatus::Running,
            job_id: None,
            attempt: 0,
            started_at: Some(now),
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output: None,
            live_preview: None,
            last_active_at: None,
        };
        let mut run = DagRun {
            id: String::new(), // assigned below
            name: cap_chars(condition, 48),
            nodes: vec![node],
            status: DagStatus::Running,
            kind: RunKind::Goal,
            max_concurrency: 1,
            fail_fast: false,
            direction: Direction::Td,
            created_at: now,
            session_id,
            completed_at: None,
            last_activity_at: now,
            error: None,
        };
        let id = {
            let mut inner = self.inner.lock();
            inner.goal_counter += 1;
            run.id = format!("goal-{}", inner.goal_counter);
            run.last_activity_at = now_ms();
            let id = run.id.clone();
            inner.runs.insert(id.clone(), run);
            id
        };
        self.emit(DagEvent::RunStatus {
            run_id: id.clone(),
            session_id: session_id_str,
            status: DagStatus::Running,
            error: None,
        });
        id
    }

    /// goal.rs hook: one loop iteration for a goal run. `iteration` lands on
    /// the node's `attempt`. `done=true` succeeds the node and completes the
    /// run (emits NodeStatus succeeded + RunStatus completed); otherwise the
    /// node stays Running with `reason` as its error and the idle clock is
    /// refreshed (emits NodeStatus running). Returns false if the run (or its
    /// main node) does not exist.
    pub fn on_goal_tick(
        &self,
        run_id: &str,
        iteration: u32,
        done: bool,
        reason: Option<String>,
    ) -> bool {
        let events = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return false;
            };
            let mut events: Vec<DagEvent> = Vec::new();
            {
                let Some(node) = run.node_mut("main") else {
                    return false;
                };
                node.attempt = iteration;
                if done {
                    node.status = NodeStatus::Succeeded;
                    node.completed_at = Some(now_ms());
                    node.error = None;
                } else {
                    node.error = reason.clone();
                }
            }
            if done {
                run.status = DagStatus::Completed;
                run.completed_at = Some(now_ms());
                emit_state(run);
                let run_session_id = run.session_id.clone().unwrap_or_default();
                events.push(DagEvent::NodeStatus {
                    run_id: run_id.to_string(),
                    session_id: run_session_id.clone(),
                    node_id: "main".to_string(),
                    status: NodeStatus::Succeeded,
                    error: None,
                });
                events.push(DagEvent::RunStatus {
                    run_id: run_id.to_string(),
                    session_id: run_session_id,
                    status: DagStatus::Completed,
                    error: None,
                });
            } else {
                emit_state(run);
                events.push(DagEvent::NodeStatus {
                    run_id: run_id.to_string(),
                    session_id: run.session_id.clone().unwrap_or_default(),
                    node_id: "main".to_string(),
                    status: NodeStatus::Running,
                    error: reason,
                });
            }
            events
        };
        for event in events {
            self.emit(event);
        }
        if done {
            self.wake_waiters(run_id);
        }
        true
    }

    /// goal.rs hook: force the run to a terminal state (Failed/Cancelled)
    /// from the outside. The main node mirrors the run status, both get
    /// `reason` + completed_at; emits NodeStatus + RunStatus.
    pub fn complete_goal(&self, run_id: &str, run_status: DagStatus, reason: Option<String>) {
        let events = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            if run.status != DagStatus::Running {
                return;
            }
            run.status = run_status.clone();
            run.completed_at = Some(now_ms());
            run.error = reason.clone();
            let mut events: Vec<DagEvent> = Vec::new();
            if let Some(node) = run.node_mut("main") {
                node.status = match run_status {
                    DagStatus::Failed => NodeStatus::Failed,
                    DagStatus::Cancelled => NodeStatus::Cancelled,
                    _ => node.status.clone(),
                };
                node.completed_at = Some(now_ms());
                node.error = reason.clone();
            }
            emit_state(run);
            if let Some(node) = run.node("main") {
                events.push(DagEvent::NodeStatus {
                    run_id: run_id.to_string(),
                    session_id: run.session_id.clone().unwrap_or_default(),
                    node_id: "main".to_string(),
                    status: node.status.clone(),
                    error: node.error.clone(),
                });
            }
            events.push(DagEvent::RunStatus {
                run_id: run_id.to_string(),
                session_id: run.session_id.clone().unwrap_or_default(),
                status: run_status,
                error: reason,
            });
            events
        };
        for event in events {
            self.emit(event);
        }
        self.wake_waiters(run_id);
    }

    pub fn get_run(&self, id: &str) -> Option<DagRun> {
        self.inner.lock().runs.get(id).cloned()
    }

    /// All runs, newest first. Ties (same-ms creation) keep registration
    /// order — mirrors the TS Map + stable sort.
    pub fn list_runs(&self) -> Vec<DagRun> {
        let mut runs: Vec<DagRun> = self.inner.lock().runs.values().cloned().collect();
        runs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| run_counter(&a.id).cmp(&run_counter(&b.id)))
        });
        runs
    }

    /// Running nodes across all running runs.
    pub fn running_node_count(&self) -> usize {
        let inner = self.inner.lock();
        inner
            .runs
            .values()
            .filter(|r| r.status == DagStatus::Running)
            .flat_map(|r| r.nodes.iter())
            .filter(|n| n.status == NodeStatus::Running)
            .count()
    }

    /// Most recently created still-running run.
    pub fn most_recent_active(&self) -> Option<DagRun> {
        let inner = self.inner.lock();
        inner
            .runs
            .values()
            .filter(|r| r.status == DagStatus::Running)
            .max_by_key(|r| r.created_at)
            .cloned()
    }

    // ── scheduling ──────────────────────────────────────────────────────────

    /// Launch every eligible ready node within the concurrency budget.
    fn tick(&self, run_id: &str) {
        loop {
            let next = {
                let inner = self.inner.lock();
                let Some(run) = inner.runs.get(run_id) else {
                    return;
                };
                if run.status != DagStatus::Running {
                    return;
                }
                let running = run
                    .nodes
                    .iter()
                    .filter(|n| n.status == NodeStatus::Running)
                    .count();
                if running >= run.max_concurrency {
                    return;
                }
                match run.nodes.iter().find(|n| n.status == NodeStatus::Ready) {
                    Some(n) => Some(n.id.clone()),
                    None => return,
                }
            };
            match next {
                Some(id) => self.start_node(run_id, &id),
                None => return,
            }
        }
    }

    fn start_node(&self, run_id: &str, node_id: &str) {
        let (launcher, token) = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            let Some(node) = run.node_mut(node_id) else {
                return;
            };
            // Concurrent ticks can both pick the same ready node; only the
            // first wins (the TS original is single-threaded).
            if node.status != NodeStatus::Ready {
                return;
            }
            node.status = NodeStatus::Running;
            node.started_at = Some(now_ms());
            node.job_id = Some(format!("job-{}-{}", run_id, node_id));
            let token = CancellationToken::new();
            emit_state(run);
            // MutexGuard does not split field borrows: touch `jobs` only
            // after the `run` borrow has ended (NLL).
            inner
                .jobs
                .insert((run_id.to_string(), node_id.to_string()), token.clone());
            (inner.launcher.clone(), token)
        };
        match launcher {
            None => {
                // No launch context (tests / misconfiguration): fail now.
                self.on_node_completed(
                    run_id,
                    node_id,
                    NodeOutcome {
                        success: false,
                        error: Some("no launch context".to_string()),
                        duration_ms: 0,
                        attempt: 0,
                        total_attempts: 0,
                        input_tokens: 0,
                        output_tokens: 0,
                        output: None,
                    },
                );
            }
            Some(launcher) => {
                let result =
                    catch_unwind(AssertUnwindSafe(|| launcher.launch(run_id, node_id, token)));
                if let Err(panic) = result {
                    self.on_node_completed(
                        run_id,
                        node_id,
                        NodeOutcome {
                            success: false,
                            error: Some(panic_message(&panic)),
                            duration_ms: 0,
                            attempt: 0,
                            total_attempts: 0,
                            input_tokens: 0,
                            output_tokens: 0,
                            output: None,
                        },
                    );
                }
            }
        }
    }

    /// Launcher callback: a node's subagent job ended with `outcome`.
    pub fn on_node_completed(&self, run_id: &str, node_id: &str, outcome: NodeOutcome) {
        let applied = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            let Some(node) = run.node_mut(node_id) else {
                return;
            };
            // Stale report (node was cancelled/skipped/retried meanwhile).
            if node.status != NodeStatus::Running {
                return;
            }
            node.completed_at = Some(now_ms());
            if outcome.success {
                node.status = NodeStatus::Succeeded;
                node.error = None;
            } else {
                node.status = NodeStatus::Failed;
                node.error = Some(
                    outcome
                        .error
                        .clone()
                        .unwrap_or_else(|| "no result".to_string()),
                );
            }
            node.result = Some(NodeResult {
                success: outcome.success,
                error: outcome.error.clone(),
                duration_ms: Some(outcome.duration_ms),
                attempt: outcome.attempt,
                total_attempts: outcome.total_attempts,
            });
            node.attempt = node.attempt.max(outcome.total_attempts);
            node.input_tokens = Some(outcome.input_tokens);
            node.output_tokens = Some(outcome.output_tokens);
            if outcome.output.is_some() {
                node.output = outcome.output;
            }
            let status = node.status.clone();
            let error = node.error.clone();
            emit_state(run);
            let event = DagEvent::NodeStatus {
                run_id: run_id.to_string(),
                session_id: run.session_id.clone().unwrap_or_default(),
                node_id: node_id.to_string(),
                status,
                error,
            };
            // `run` borrow ends here (NLL) — jobs map is a separate field.
            inner
                .jobs
                .remove(&(run_id.to_string(), node_id.to_string()));
            (true, event)
        };
        if applied.0 {
            self.emit(applied.1);
            self.after_node_terminal(run_id, node_id);
        }
    }

    /// Live token/preview sync while a node is running (mirrors the TS job
    /// update handler; refreshes the idle watchdog clock).
    pub fn on_node_update(
        &self,
        run_id: &str,
        node_id: &str,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        preview: Option<String>,
    ) {
        let mut inner = self.inner.lock();
        let Some(run) = inner.runs.get_mut(run_id) else {
            return;
        };
        run.last_activity_at = now_ms();
        let Some(node) = run.node_mut(node_id) else {
            return;
        };
        if let Some(t) = input_tokens {
            node.input_tokens = Some(t);
        }
        if let Some(t) = output_tokens {
            node.output_tokens = Some(t);
        }
        if let Some(p) = preview {
            node.live_preview = Some(cap_chars(&p, 2048));
        }
        node.last_active_at = Some(now_ms());
    }

    /// Re-derive non-terminal node states after a dependency flipped.
    fn reconcile(&self, run_id: &str) {
        let mut inner = self.inner.lock();
        let Some(run) = inner.runs.get_mut(run_id) else {
            return;
        };
        super::graph::reconcile(run);
        emit_state(run);
    }

    /// Common post-terminal-node processing: failFast abort, cascade,
    /// schedule, maybe finish.
    fn after_node_terminal(&self, run_id: &str, node_id: &str) {
        let (fail_fast, failed) = {
            let inner = self.inner.lock();
            let Some(run) = inner.runs.get(run_id) else {
                return;
            };
            let failed = run
                .node(node_id)
                .map(|n| n.status == NodeStatus::Failed)
                .unwrap_or(false);
            (run.fail_fast, failed)
        };
        if failed && fail_fast {
            self.cancel_run(
                run_id,
                Some(&format!("failFast: 节点 {node_id} 失败, 终止整个运行")),
            );
            return;
        }
        self.reconcile(run_id);
        self.tick(run_id);
        self.maybe_complete(run_id);
    }

    /// Terminal when every node is terminal; sets run status failed/completed.
    pub fn maybe_complete(&self, run_id: &str) {
        let terminal = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            if run.status != DagStatus::Running || !run.nodes.iter().all(|n| is_terminal(&n.status))
            {
                None
            } else {
                let has_failure = run
                    .nodes
                    .iter()
                    .any(|n| matches!(n.status, NodeStatus::Failed | NodeStatus::Cancelled));
                run.status = if has_failure {
                    DagStatus::Failed
                } else {
                    DagStatus::Completed
                };
                run.completed_at = Some(now_ms());
                emit_state(run);
                Some(DagEvent::RunStatus {
                    run_id: run_id.to_string(),
                    session_id: run.session_id.clone().unwrap_or_default(),
                    status: run.status.clone(),
                    error: run.error.clone(),
                })
            }
        };
        if let Some(event) = terminal {
            self.emit(event);
            self.wake_waiters(run_id);
        }
    }

    // ── intervention ────────────────────────────────────────────────────────

    /// Abort the whole run: in-flight jobs killed, pending/ready cancelled.
    pub fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        let (cancelled, session_id) = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            if run.status != DagStatus::Running {
                return;
            }
            run.error = Some(reason.unwrap_or("cancelled by orchestrator").to_string());
            let mut running_ids: Vec<String> = Vec::new();
            for node in &mut run.nodes {
                match node.status {
                    NodeStatus::Running => {
                        node.status = NodeStatus::Cancelled;
                        node.completed_at = Some(now_ms());
                        node.error = run.error.clone();
                        running_ids.push(node.id.clone());
                    }
                    NodeStatus::Pending | NodeStatus::Ready => {
                        node.status = NodeStatus::Cancelled;
                        node.completed_at = Some(now_ms());
                        node.error = run.error.clone();
                    }
                    _ => {}
                }
            }
            run.status = DagStatus::Cancelled;
            run.completed_at = Some(now_ms());
            emit_state(run);
            (running_ids, run.session_id.clone().unwrap_or_default())
        };
        // Second lock scope: abort the collected jobs' tokens.
        {
            let mut inner = self.inner.lock();
            for id in cancelled {
                if let Some(token) = inner.jobs.remove(&(run_id.to_string(), id)) {
                    token.cancel();
                }
            }
        }
        self.emit(DagEvent::RunStatus {
            run_id: run_id.to_string(),
            session_id,
            status: DagStatus::Cancelled,
            error: Some(reason.unwrap_or("cancelled by orchestrator").to_string()),
        });
        self.wake_waiters(run_id);
    }

    /// Re-run blocked nodes. Without node_ids: all failed+cancelled nodes.
    /// With node_ids: those nodes plus their blocked downstream closure.
    /// Also restarts a terminal run. Returns the reset node ids.
    pub fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String> {
        let to_reset = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return Vec::new();
            };
            let targets: Vec<String> = match node_ids {
                Some(ids) if !ids.is_empty() => ids.to_vec(),
                _ => run
                    .nodes
                    .iter()
                    .filter(|n| is_blocked(&n.status))
                    .map(|n| n.id.clone())
                    .collect(),
            };
            let mut to_reset: Vec<String> = Vec::new();
            for id in targets {
                let Some(node) = run.node(&id) else {
                    continue;
                };
                if !is_blocked(&node.status) {
                    continue;
                }
                push_unique(&mut to_reset, &id);
                for cid in downstream_closure(&run.nodes, &id) {
                    let Some(c) = run.node(&cid) else {
                        continue;
                    };
                    if is_blocked(&c.status) {
                        push_unique(&mut to_reset, &cid);
                    }
                }
            }
            for id in &to_reset {
                if let Some(n) = run.node_mut(id) {
                    reset_node(n);
                }
            }
            if run.status != DagStatus::Running {
                run.status = DagStatus::Running;
                run.completed_at = None;
                run.error = None;
            }
            emit_state(run);
            to_reset
        };
        self.reconcile(run_id);
        self.tick(run_id);
        self.maybe_complete(run_id);
        to_reset
    }

    /// Mark a node skipped (counts as success for downstream). Skipping a
    /// failed node also releases its cancelled downstream closure (same
    /// replay semantics as retry, but the node itself is not re-run).
    pub fn skip(&self, run_id: &str, node_id: &str) -> bool {
        let to_abort = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return false;
            };
            let Some(node) = run.node(node_id) else {
                return false;
            };
            if matches!(node.status, NodeStatus::Succeeded | NodeStatus::Skipped) {
                return false;
            }
            if node.status == NodeStatus::Failed {
                let closure = downstream_closure(&run.nodes, node_id);
                for cid in closure {
                    if let Some(c) = run.node_mut(&cid) {
                        if is_blocked(&c.status) {
                            reset_node(c);
                        }
                    }
                }
            }
            let was_running = run
                .node(node_id)
                .is_some_and(|n| n.status == NodeStatus::Running);
            let Some(node) = run.node_mut(node_id) else {
                return false;
            };
            node.status = NodeStatus::Skipped;
            node.completed_at = Some(now_ms());
            node.error = Some(
                if was_running {
                    "skipped by orchestrator (job aborted)"
                } else {
                    "skipped by orchestrator"
                }
                .to_string(),
            );
            let error = node.error.clone();
            if run.status != DagStatus::Running {
                run.status = DagStatus::Running;
                run.completed_at = None;
                run.error = None;
            }
            emit_state(run);
            let event = DagEvent::NodeStatus {
                run_id: run_id.to_string(),
                session_id: run.session_id.clone().unwrap_or_default(),
                node_id: node_id.to_string(),
                status: NodeStatus::Skipped,
                error,
            };
            // `run` borrow ends here (NLL) — jobs map is a separate field.
            let token = if was_running {
                inner
                    .jobs
                    .remove(&(run_id.to_string(), node_id.to_string()))
            } else {
                None
            };
            (token, event)
        };
        if let Some(token) = to_abort.0 {
            token.cancel();
        }
        self.emit(to_abort.1);
        self.after_node_terminal(run_id, node_id);
        true
    }

    /// Abort all still-running runs (session shutdown). Returns the count.
    pub fn abort_all_runs(&self, reason: &str) -> usize {
        let ids: Vec<String> = {
            let inner = self.inner.lock();
            inner
                .runs
                .iter()
                .filter(|(_, r)| r.status == DagStatus::Running)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in &ids {
            self.cancel_run(id, Some(reason));
        }
        ids.len()
    }

    /// Resume persisted runs (running nodes demoted to ready and re-scheduled
    /// by tick). Skips ids already registered; aligns the dag-N counter.
    /// Returns the restored run ids.
    pub fn restore(&self, runs: Vec<PersistedRun>) -> Vec<String> {
        let mut restored: Vec<String> = Vec::new();
        {
            let mut inner = self.inner.lock();
            for p in runs {
                if inner.runs.contains_key(&p.id) {
                    continue;
                }
                let run = hydrate(p);
                inner.dag_counter = inner
                    .dag_counter
                    .max(max_run_counter(std::slice::from_ref(&run)));
                let id = run.id.clone();
                inner.runs.insert(id.clone(), run);
                restored.push(id);
            }
        }
        for id in &restored {
            self.reconcile(id);
            self.tick(id);
        }
        restored
    }

    // ── waiting ─────────────────────────────────────────────────────────────

    /// Block until each run reaches a terminal state (event-driven, no
    /// polling). One shared deadline for all runs; `(id, true)` means the run
    /// was still running when the timeout / idle watchdog fired.
    pub async fn wait_for_runs(
        &self,
        run_ids: &[String],
        timeout: Duration,
        idle: Option<Duration>,
    ) -> Vec<(String, bool)> {
        if run_ids.is_empty() {
            return Vec::new();
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let waiters: Vec<_> = run_ids
            .iter()
            .map(|id| {
                let engine = self.clone();
                let id = id.clone();
                async move { engine.wait_for_run(&id, remaining, idle).await }
            })
            .collect();
        join_all(waiters).await
    }

    /// Condition-variable loop, deadline-bounded; re-checks terminal state
    /// every wake so a missed notify (notify_waiters raced our park) is
    /// harmless — we fall through to the next sleep.
    async fn wait_for_run(
        &self,
        run_id: &str,
        timeout: Duration,
        idle: Option<Duration>,
    ) -> (String, bool) {
        self.maybe_complete(run_id);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let terminal = {
                let inner = self.inner.lock();
                match inner.runs.get(run_id) {
                    Some(run) => run.status != DagStatus::Running,
                    // Unknown run: treat as already finished (defensive close).
                    None => true,
                }
            };
            if terminal {
                return (run_id.to_string(), false);
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return (run_id.to_string(), true);
            }
            let last_activity = {
                let inner = self.inner.lock();
                inner
                    .runs
                    .get(run_id)
                    .map(|r| r.last_activity_at)
                    .unwrap_or(now_ms())
            };
            let mut sleep = deadline - now;
            if let Some(idle_dur) = idle {
                let idle_ms = idle_dur.as_millis() as i64;
                let idle_elapsed = (now_ms() - last_activity).max(0);
                if idle_elapsed >= idle_ms {
                    return (run_id.to_string(), true);
                }
                sleep = sleep.min(Duration::from_millis((idle_ms - idle_elapsed) as u64));
            }
            let notify = self
                .inner
                .lock()
                .waiters
                .entry(run_id.to_string())
                .or_default()
                .clone();
            tokio::select! {
                _ = tokio::time::sleep(sleep) => {}
                _ = notify.notified() => {}
            }
        }
    }

    fn wake_waiters(&self, run_id: &str) {
        let notify = self.inner.lock().waiters.get(run_id).cloned();
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }

    /// Test-only: clear all engine state.
    pub fn __reset_for_tests(&self) {
        let mut inner = self.inner.lock();
        inner.runs.clear();
        inner.jobs.clear();
        inner.waiters.clear();
        inner.dag_counter = 0;
        inner.goal_counter = 0;
        inner.events = None;
        inner.launcher = None;
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// TS `emitState` equivalent: stamp last activity (drives the idle watchdog).
/// State listeners (widget push) are a p3 concern, not wired here yet.
fn emit_state(run: &mut DagRun) {
    run.last_activity_at = now_ms();
}

/// Revert a blocked node to pending (retry/skip replay).
fn reset_node(n: &mut DagNode) {
    n.status = NodeStatus::Pending;
    n.started_at = None;
    n.completed_at = None;
    n.error = None;
    n.job_id = None;
    n.result = None;
    n.attempt = 0;
    n.input_tokens = None;
    n.output_tokens = None;
}

fn push_unique(vec: &mut Vec<String>, id: &str) {
    if !vec.iter().any(|v| v == id) {
        vec.push(id.to_string());
    }
}

/// "dag-12" → 12 (0 for anything else) — list_runs tie-breaker.
fn run_counter(id: &str) -> u64 {
    id.strip_prefix("dag-")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Char-safe truncation for live previews (TS `updatePreview` caps ~2 KB).
fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect()
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "launcher panicked".to_string()
    }
}

#[cfg(test)]
// Test files live in `tests/runtime/graph_engineering/engine/` (mirror of
// `src/runtime/graph_engineering/`), pulled in by path so they keep unit-test
// semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge!("../../../tests/runtime/graph_engineering/engine/mod.rs");
