//! DAG orchestration engine entry: run registry, goal coordination, and
//! retry/skip/cancel operations. Execution is delegated to a `NodeLauncher`.
//!
//! Scheduling and run-lifecycle methods live in the sibling `scheduler`
//! module; lock-local transition helpers live in `engine_state`.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::observability::{OperationScope, RuntimeObserver, noop_runtime_observer};

use super::engine_state::{cap_chars, emit_state, push_unique, reset_node, run_counter};
use super::model::{build_run, downstream_closure, is_blocked, now_ms, validate_graph};
use super::persist::DagPersistSink;
// Glob-imported by the bridged tests (`tests/multiagent/graph/engine/`).
#[cfg(test)]
use super::persist::PersistedRun;
use super::types::{
    DagEvent, DagNode, DagRun, DagRunDef, DagStatus, Direction, NodeStatus, RunKind,
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
    pub(super) inner: Arc<Mutex<EngineInner>>,
    pub(super) observer: Arc<dyn RuntimeObserver>,
    pub(super) run_operations: Arc<Mutex<HashMap<String, OperationScope>>>,
    pub(super) node_operations: Arc<Mutex<HashMap<(String, String), OperationScope>>>,
    /// Persistence sink (app-layer debounced writer), stored OUTSIDE the
    /// engine lock so `notify_persist` can fire from any state-change point
    /// without deadlocking (parking_lot is not reentrant).
    persist_sink: Arc<parking_lot::Mutex<Option<Arc<dyn DagPersistSink>>>>,
}

pub(super) struct EngineInner {
    pub(super) runs: HashMap<String, DagRun>,
    pub(super) dag_counter: u64,
    /// Independent id sequence for goal runs (goal-N, self-loop semantics).
    pub(super) goal_counter: u64,
    pub(super) launcher: Option<Arc<dyn NodeLauncher>>,
    /// Session-scoped launcher overrides. Exact match on `DagRun.session_id`
    /// wins over the global launcher (session-less runs may use the `None` key).
    pub(super) session_launchers: HashMap<Option<String>, Arc<dyn NodeLauncher>>,
    /// Event-plane broadcast (node_status / run_status). `None` = detached.
    pub(super) events: Option<tokio::sync::broadcast::Sender<DagEvent>>,
    /// Abort tokens for in-flight nodes, keyed by (run_id, node_id).
    pub(super) jobs: HashMap<(String, String), CancellationToken>,
    /// Condition variable per run for `wait_for_runs` (created on demand).
    pub(super) waiters: HashMap<String, Arc<Notify>>,
}

impl Clone for DagEngine {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            observer: Arc::clone(&self.observer),
            run_operations: Arc::clone(&self.run_operations),
            node_operations: Arc::clone(&self.node_operations),
            persist_sink: Arc::clone(&self.persist_sink),
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
        Self::with_observer(noop_runtime_observer())
    }

    pub fn with_observer(observer: Arc<dyn RuntimeObserver>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(EngineInner {
                runs: HashMap::new(),
                dag_counter: 0,
                goal_counter: 0,
                launcher: None,
                session_launchers: HashMap::new(),
                events: None,
                jobs: HashMap::new(),
                waiters: HashMap::new(),
            })),
            observer,
            run_operations: Arc::new(Mutex::new(HashMap::new())),
            node_operations: Arc::new(Mutex::new(HashMap::new())),
            persist_sink: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the node launcher (tests inject a fake one).
    pub fn set_launcher(&self, launcher: Option<Arc<dyn NodeLauncher>>) {
        self.inner.lock().launcher = launcher;
    }

    /// Register the launcher owned by one session; exact matches precede the
    /// global compatibility launcher. `None` owns session-less runs.
    pub fn set_session_launcher(
        &self,
        session_id: Option<String>,
        launcher: Arc<dyn NodeLauncher>,
    ) {
        self.inner
            .lock()
            .session_launchers
            .insert(session_id, launcher);
    }

    /// Wire the event-plane broadcast (transport setup calls this once);
    /// `None` detaches (same contract as SubagentJobRegistry).
    pub fn set_event_sender(&self, tx: Option<tokio::sync::broadcast::Sender<DagEvent>>) {
        self.inner.lock().events = tx;
    }

    /// Wire the persistence sink (app layer debounces + writes). `None` = no
    /// persistence (tests, embedders).
    pub fn set_persist_sink(&self, sink: Option<Arc<dyn DagPersistSink>>) {
        *self.persist_sink.lock() = sink;
    }

    /// Non-blocking dirty notification to the persistence sink (if any).
    /// Takes only the sink lock (never the engine lock), so it is safe to
    /// call from any state-change point, including while `inner` is held.
    pub(super) fn notify_persist(&self) {
        if let Some(sink) = self.persist_sink.lock().clone() {
            sink.notify_dirty();
        }
    }

    /// Broadcast an event-plane message (no-op without a sender).
    pub(super) fn emit(&self, event: DagEvent) {
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
        self.begin_run_observation(&run.id);
        self.reconcile(&run.id);
        self.tick(&run.id);
        self.notify_persist();
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
            max_iterations: None,
            tools: None,
            status: NodeStatus::Running,
            job_id: None,
            attempt: 0,
            launch_gen: 0,
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
        self.begin_run_observation(&id);
        self.begin_node_observation(&id, "main");
        self.emit(DagEvent::RunStatus {
            run_id: id.clone(),
            session_id: session_id_str,
            status: DagStatus::Running,
            error: None,
        });
        self.notify_persist();
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
        self.notify_persist();
        if done {
            self.finish_node_observation(run_id, "main", NodeStatus::Succeeded);
            self.finish_run_observation(run_id, DagStatus::Completed);
            self.wake_waiters(run_id);
        }
        true
    }

    /// goal.rs hook: record the evaluator job id on the goal run's `main` node.
    /// Goal nodes are hook-driven (never engine-dispatched), so this is what
    /// links the graph surface to the evaluator's registry job / transcript.
    pub fn on_goal_evaluator_finished(&self, run_id: &str, job_id: String) {
        let mut inner = self.inner.lock();
        if let Some(run) = inner.runs.get_mut(run_id) {
            if let Some(node) = run.node_mut("main") {
                node.job_id = Some(job_id);
            }
        }
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
        if let Some(run) = self.get_run(run_id) {
            if let Some(node) = run.node("main") {
                self.finish_node_observation(run_id, "main", node.status.clone());
            }
            self.finish_run_observation(run_id, run.status);
        }
        self.notify_persist();
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

    /// Re-home every run owned by `from_session` to `to_session`. Returns the
    /// number of runs moved. Used by collapse `--adopt` / `attach_runs`.
    pub fn rehome_runs(&self, from_session: &str, to_session: &str) -> usize {
        let count = {
            let mut inner = self.inner.lock();
            let mut count = 0;
            for run in inner.runs.values_mut() {
                if run.session_id.as_deref() == Some(from_session) {
                    run.session_id = Some(to_session.to_string());
                    run.last_activity_at = now_ms();
                    count += 1;
                }
            }
            count
        };
        if count > 0 {
            self.notify_persist();
        }
        count
    }

    // ── intervention ────────────────────────────────────────────────────────

    /// Abort the whole run: in-flight jobs killed, pending/ready cancelled.
    pub fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        let (running, cancelled, session_id) = {
            let mut inner = self.inner.lock();
            let Some(run) = inner.runs.get_mut(run_id) else {
                return;
            };
            if run.status != DagStatus::Running {
                return;
            }
            run.error = Some(reason.unwrap_or("cancelled by orchestrator").to_string());
            let mut running_ids: Vec<String> = Vec::new();
            let mut cancelled_ids: Vec<String> = Vec::new();
            for node in &mut run.nodes {
                match node.status {
                    NodeStatus::Running => {
                        node.status = NodeStatus::Cancelled;
                        node.completed_at = Some(now_ms());
                        node.error = run.error.clone();
                        running_ids.push(node.id.clone());
                        cancelled_ids.push(node.id.clone());
                    }
                    NodeStatus::Pending | NodeStatus::Ready => {
                        node.status = NodeStatus::Cancelled;
                        node.completed_at = Some(now_ms());
                        node.error = run.error.clone();
                        cancelled_ids.push(node.id.clone());
                    }
                    _ => {}
                }
            }
            run.status = DagStatus::Cancelled;
            run.completed_at = Some(now_ms());
            emit_state(run);
            (
                running_ids,
                cancelled_ids,
                run.session_id.clone().unwrap_or_default(),
            )
        };
        // Second lock scope: abort the collected jobs' tokens.
        {
            let mut inner = self.inner.lock();
            for id in &running {
                if let Some(token) = inner.jobs.remove(&(run_id.to_string(), id.clone())) {
                    token.cancel();
                }
            }
        }
        for node_id in &cancelled {
            self.begin_node_observation(run_id, node_id);
            self.finish_node_observation(run_id, node_id, NodeStatus::Cancelled);
        }
        self.finish_run_observation(run_id, DagStatus::Cancelled);
        self.emit(DagEvent::RunStatus {
            run_id: run_id.to_string(),
            session_id,
            status: DagStatus::Cancelled,
            error: Some(reason.unwrap_or("cancelled by orchestrator").to_string()),
        });
        self.notify_persist();
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
        self.begin_run_observation(run_id);
        self.reconcile(run_id);
        self.tick(run_id);
        self.maybe_complete(run_id);
        self.notify_persist();
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
        self.begin_node_observation(run_id, node_id);
        self.finish_node_observation(run_id, node_id, NodeStatus::Skipped);
        self.emit(to_abort.1);
        self.after_node_terminal(run_id, node_id);
        self.notify_persist();
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

    /// Test-only: clear all engine state.
    pub fn __reset_for_tests(&self) {
        {
            let mut inner = self.inner.lock();
            inner.runs.clear();
            inner.jobs.clear();
            inner.waiters.clear();
            inner.dag_counter = 0;
            inner.goal_counter = 0;
            inner.events = None;
            inner.launcher = None;
            inner.session_launchers.clear();
        }
        let run_scopes: Vec<_> = self
            .run_operations
            .lock()
            .drain()
            .map(|(_, scope)| scope)
            .collect();
        let node_scopes: Vec<_> = self
            .node_operations
            .lock()
            .drain()
            .map(|(_, scope)| scope)
            .collect();
        drop(run_scopes);
        drop(node_scopes);
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/engine");
