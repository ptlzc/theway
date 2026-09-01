//! Scheduling / run-lifecycle methods of [`DagEngine`]: node dispatch
//! (`tick`/`start_node`), launcher callbacks (`on_node_completed` /
//! `on_node_update`), post-terminal propagation, run completion, persisted
//! restore, and the event-driven wait loop.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use futures::future::join_all;
use tokio_util::sync::CancellationToken;

use super::engine::{DagEngine, NodeOutcome};
use super::engine_state::{cap_chars, emit_state, panic_message};
use super::model::{is_terminal, now_ms};
use super::persist::{PersistedRun, hydrate, max_run_counter};
use super::types::{DagEvent, DagStatus, NodeResult, NodeStatus};

impl DagEngine {
    // ── scheduling ──────────────────────────────────────────────────────────

    /// Launch every eligible ready node within the concurrency budget.
    pub(super) fn tick(&self, run_id: &str) {
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
            node.launch_gen += 1;
            node.started_at = Some(now_ms());
            node.job_id = Some(format!("job-{}-{}", run_id, node_id));
            let token = CancellationToken::new();
            emit_state(run);
            let session_id = run.session_id.clone();
            // MutexGuard does not split field borrows: touch `jobs` only
            // after the `run` borrow has ended (NLL).
            inner
                .jobs
                .insert((run_id.to_string(), node_id.to_string()), token.clone());
            let launcher = inner
                .session_launchers
                .get(&session_id)
                .cloned()
                .or_else(|| inner.launcher.clone());
            (launcher, token)
        };
        self.begin_node_observation(run_id, node_id);
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
                status: status.clone(),
                error,
            };
            // `run` borrow ends here (NLL) — jobs map is a separate field.
            inner
                .jobs
                .remove(&(run_id.to_string(), node_id.to_string()));
            (event, status)
        };
        self.finish_node_observation(run_id, node_id, applied.1);
        self.emit(applied.0);
        self.after_node_terminal(run_id, node_id);
        self.notify_persist();
    }

    /// Live token/preview sync while a node is running (mirrors the TS job
    /// update handler; refreshes the idle watchdog clock). `launch_gen` is the
    /// launch generation the reporting job captured at dispatch time: updates
    /// from a stale job (its launch was cancelled/skipped/retried meanwhile)
    /// are dropped so they can't pollute a re-launched attempt's tokens/preview
    /// or refresh the run's idle clock.
    pub fn on_node_update(
        &self,
        run_id: &str,
        node_id: &str,
        launch_gen: u64,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        preview: Option<String>,
    ) {
        let mut inner = self.inner.lock();
        let Some(run) = inner.runs.get_mut(run_id) else {
            return;
        };
        let Some(node) = run.node_mut(node_id) else {
            return;
        };
        if node.launch_gen != launch_gen {
            return;
        }
        let now = now_ms();
        if let Some(t) = input_tokens {
            node.input_tokens = Some(t);
        }
        if let Some(t) = output_tokens {
            node.output_tokens = Some(t);
        }
        if let Some(p) = preview {
            node.live_preview = Some(cap_chars(&p, 2048));
        }
        node.last_active_at = Some(now);
        // After the `node` borrow ends (NLL): refresh the run idle clock.
        run.last_activity_at = now;
        drop(inner);
        self.notify_persist();
    }

    /// Re-derive non-terminal node states after a dependency flipped.
    pub(super) fn reconcile(&self, run_id: &str) {
        let mut inner = self.inner.lock();
        let Some(run) = inner.runs.get_mut(run_id) else {
            return;
        };
        super::model::reconcile(run);
        emit_state(run);
    }

    /// Common post-terminal-node processing: failFast abort, cascade,
    /// schedule, maybe finish.
    pub(super) fn after_node_terminal(&self, run_id: &str, node_id: &str) {
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
                Some((
                    DagEvent::RunStatus {
                        run_id: run_id.to_string(),
                        session_id: run.session_id.clone().unwrap_or_default(),
                        status: run.status.clone(),
                        error: run.error.clone(),
                    },
                    run.status.clone(),
                    run.session_id.clone(),
                ))
            }
        };
        if let Some((event, status, session_id)) = terminal {
            self.finish_run_observation(run_id, status);
            self.emit(event);
            self.evict(session_id.as_deref());
            self.wake_waiters(run_id);
        }
        self.notify_persist();
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
            if self
                .get_run(id)
                .is_some_and(|run| run.status == DagStatus::Running)
            {
                self.begin_run_observation(id);
            }
            self.reconcile(id);
            self.tick(id);
        }
        if !restored.is_empty() {
            self.notify_persist();
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

    pub(super) fn wake_waiters(&self, run_id: &str) {
        let notify = self.inner.lock().waiters.get(run_id).cloned();
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("multiagent/graph/scheduler");
