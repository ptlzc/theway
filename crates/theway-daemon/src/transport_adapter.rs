//! Adapters from daemon/core runtime state to transport-owned operation seams
//! and DTOs. Protocol servers never need to construct or inspect the concrete
//! DAG engine or job registry.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::persist::{PersistedRun, to_persisted};
use theway_core::multiagent::graph::types::{
    DagEvent, DagNode, DagRun, DagStatus, Direction, NodeResult, NodeStatus, RunKind,
};
use theway_core::multiagent::jobs::{
    SubagentJob, SubagentJobEvent, SubagentJobRegistry, SubagentJobStatus,
};
use theway_transport::transport::{GraphOps, JobOps};
use theway_transport::wire::{
    WireAgentEvent, WireAgentJobSnapshot, WireDagEvent, WireDagNodeSnapshot, WireDagRunSnapshot,
    WireGraphCheckpoint, WireGraphKind, WireNodeOutput, WireNodeResultSnapshot,
};

#[derive(Clone)]
pub struct CoreJobOps {
    registry: SubagentJobRegistry,
    engine: Arc<DagEngine>,
}

impl CoreJobOps {
    pub fn new(registry: SubagentJobRegistry, engine: Arc<DagEngine>) -> Self {
        Self { registry, engine }
    }
}

impl JobOps for CoreJobOps {
    fn node_output(&self, run_id: &str, node_id: &str) -> WireNodeOutput {
        let session_id = self.engine.get_run(run_id).and_then(|run| run.session_id);
        let messages = match session_id.as_deref() {
            Some(session_id) => {
                self.registry
                    .node_messages_for_session(Some(session_id), run_id, node_id)
            }
            None => self.registry.node_messages(run_id, node_id),
        };
        let job = self.registry.find_node(run_id, node_id);
        WireNodeOutput {
            output: job.as_ref().map(|job| job.output.clone()),
            truncated: job.as_ref().is_some_and(|job| job.truncated),
            messages,
            messages_truncated: job.is_some_and(|job| job.messages_truncated),
        }
    }

    fn interrupt_node(&self, run_id: &str, node_id: &str) -> bool {
        self.registry.interrupt_node(run_id, node_id)
    }

    fn steer_node(&self, run_id: &str, node_id: &str, text: String) -> bool {
        self.registry.steer_node(run_id, node_id, text)
    }
}

#[derive(Clone)]
pub struct CoreGraphOps {
    engine: Arc<DagEngine>,
}

impl CoreGraphOps {
    pub fn new(engine: Arc<DagEngine>) -> Self {
        Self { engine }
    }
}

impl GraphOps for CoreGraphOps {
    fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        self.engine.cancel_run(run_id, reason);
    }

    fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String> {
        self.engine.retry(run_id, node_ids)
    }

    fn skip(&self, run_id: &str, node_id: &str) -> bool {
        self.engine.skip(run_id, node_id)
    }

    fn checkpoints(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> Result<Vec<WireGraphCheckpoint>> {
        let runs: Vec<DagRun> = match run_id {
            Some(run_id) => self.engine.get_run(run_id).into_iter().collect(),
            None => self.engine.list_runs(),
        };
        runs.into_iter()
            .filter(|run| {
                run.session_id
                    .as_deref()
                    .is_none_or(|owner| owner == session_id)
            })
            .map(|run| {
                let snapshot = serde_json::to_string(&to_persisted(&run))?;
                Ok(WireGraphCheckpoint {
                    kind: match run.kind {
                        RunKind::Goal => WireGraphKind::Goal,
                        RunKind::Dag => WireGraphKind::Dag,
                    },
                    run_id: run.id,
                    snapshot,
                })
            })
            .collect()
    }

    fn restore(&self, session_id: &str, snapshot: &str) -> Result<String> {
        let mut persisted: PersistedRun =
            serde_json::from_str(snapshot).map_err(|error| anyhow!("invalid snapshot: {error}"))?;
        persisted.session_id = Some(session_id.to_string());
        self.engine
            .restore(vec![persisted])
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("restore produced no run"))
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.engine
            .list_runs()
            .into_iter()
            .filter(|run| run.session_id.as_deref() == Some(session_id))
            .map(|run| dag_run_snapshot(&run))
            .collect()
    }

    fn clear_session_runs(&self, session_id: Option<&str>, keep: usize) -> usize {
        self.engine.clear_session_runs(session_id, keep)
    }
}

pub fn dag_run_snapshot(run: &DagRun) -> WireDagRunSnapshot {
    WireDagRunSnapshot {
        id: run.id.clone(),
        name: run.name.clone(),
        kind: run.kind.as_str().to_string(),
        status: dag_status_str(&run.status).to_string(),
        fail_fast: run.fail_fast,
        max_concurrency: run.max_concurrency,
        direction: match run.direction {
            Direction::Td => "TD".to_string(),
            Direction::Lr => "LR".to_string(),
        },
        created_at: run.created_at,
        completed_at: run.completed_at,
        error: run.error.clone(),
        nodes: run.nodes.iter().map(dag_node_snapshot).collect(),
    }
}

fn dag_node_snapshot(node: &DagNode) -> WireDagNodeSnapshot {
    WireDagNodeSnapshot {
        id: node.id.clone(),
        agent: node.agent.clone(),
        status: node_status_str(&node.status).to_string(),
        depends_on: node.depends_on.clone(),
        job_id: node.job_id.clone(),
        attempt: node.attempt,
        started_at: node.started_at,
        completed_at: node.completed_at,
        error: node.error.clone(),
        input_tokens: node.input_tokens,
        output_tokens: node.output_tokens,
        result: node.result.as_ref().map(node_result_snapshot),
        output_tail: node.output.clone(),
        live_preview: node.live_preview.clone(),
    }
}

fn node_result_snapshot(result: &NodeResult) -> WireNodeResultSnapshot {
    WireNodeResultSnapshot {
        success: result.success,
        error: result.error.clone(),
        duration_ms: result.duration_ms,
        attempt: result.attempt,
        total_attempts: result.total_attempts,
    }
}

pub fn subagent_job_snapshot(job: &SubagentJob) -> WireAgentJobSnapshot {
    WireAgentJobSnapshot {
        id: job.id.clone(),
        agent: job.agent.clone(),
        source: job.source.clone(),
        run_id: job.run_id.clone(),
        node_id: job.node_id.clone(),
        status: job.status.as_str().to_string(),
        started_at: job.started_at,
        completed_at: job.completed_at,
        duration_ms: job
            .completed_at
            .zip(job.started_at)
            .map(|(end, start)| (end - start).max(0) as u64),
        attempt: job.attempt,
        total_attempts: job.total_attempts,
        input_tokens: Some(job.input_tokens),
        output_tokens: Some(job.output_tokens),
        error: job.error.clone(),
        output_tail: Some(job.output.clone()),
        live_preview: (job.status == SubagentJobStatus::Running).then(|| job.output.clone()),
        tps: job.tps(),
        cps: job.cps(),
        chars: Some(job.chars),
        tools_called: Some(job.tools_called),
        turn: Some(job.turn),
    }
}

pub fn agent_event(event: SubagentJobEvent) -> WireAgentEvent {
    match event {
        SubagentJobEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
            session_id,
        } => WireAgentEvent::Started {
            id,
            agent,
            source,
            run_id,
            node_id,
            session_id: session_id.unwrap_or_default(),
        },
        SubagentJobEvent::Output {
            id,
            chunk,
            session_id,
        } => WireAgentEvent::Output {
            id,
            chunk,
            session_id: session_id.unwrap_or_default(),
        },
        SubagentJobEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
            session_id,
        } => WireAgentEvent::Metrics {
            id,
            tps,
            cps,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            turn,
            session_id: session_id.unwrap_or_default(),
        },
        SubagentJobEvent::Completed {
            id,
            status,
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            session_id,
        } => WireAgentEvent::Completed {
            id,
            status: status.as_str().to_string(),
            error,
            chars,
            tokens_in,
            tokens_out,
            tools_called,
            session_id: session_id.unwrap_or_default(),
        },
    }
}

pub fn dag_event(event: DagEvent) -> WireDagEvent {
    match event {
        DagEvent::NodeStatus {
            run_id,
            session_id,
            node_id,
            status,
            error,
        } => WireDagEvent::NodeStatus {
            run_id,
            session_id,
            node_id,
            status: node_status_str(&status).to_string(),
            error,
        },
        DagEvent::RunStatus {
            run_id,
            session_id,
            status,
            error,
        } => WireDagEvent::RunStatus {
            run_id,
            session_id,
            status: dag_status_str(&status).to_string(),
            error,
        },
    }
}

fn node_status_str(status: &NodeStatus) -> &'static str {
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

fn dag_status_str(status: &DagStatus) -> &'static str {
    match status {
        DagStatus::Running => "running",
        DagStatus::Completed => "completed",
        DagStatus::Failed => "failed",
        DagStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_core::multiagent::jobs::SubagentJobInit;

    #[test]
    fn job_ops_projects_output_and_messages() {
        let registry = SubagentJobRegistry::new();
        let id = registry.register(SubagentJobInit {
            agent: "explorer".into(),
            source: "dag".into(),
            run_id: Some("run-1".into()),
            node_id: Some("node-1".into()),
            session_id: Some("session-1".into()),
        });
        registry.update(&id, |job| {
            job.output = "result".into();
            job.messages.push(serde_json::json!({"role": "assistant"}));
            job.messages_truncated = true;
        });

        let output =
            CoreJobOps::new(registry, Arc::new(DagEngine::new())).node_output("run-1", "node-1");

        assert_eq!(output.output.as_deref(), Some("result"));
        assert_eq!(output.messages.unwrap().len(), 1);
        assert!(output.messages_truncated);
    }

    #[test]
    fn job_ops_uses_runs_exact_session_when_run_node_ids_collide() {
        let engine = Arc::new(DagEngine::new());
        let run_id = engine.plan_goal("shared", Some("session-a".into()));
        let registry = SubagentJobRegistry::new();
        for (session_id, text) in [("session-a", "from-a"), ("session-b", "from-b")] {
            let id = registry.register(SubagentJobInit {
                agent: "explorer".into(),
                source: "dag".into(),
                run_id: Some(run_id.clone()),
                node_id: Some("node-1".into()),
                session_id: Some(session_id.into()),
            });
            registry.update(&id, |job| {
                job.messages.push(serde_json::json!({ "text": text }));
            });
        }

        let output = CoreJobOps::new(registry, engine).node_output(&run_id, "node-1");

        let messages = output.messages.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], "from-a");
    }

    #[test]
    fn agent_event_maps_session_ownership() {
        let events = [
            SubagentJobEvent::Started {
                id: "job-1".into(),
                agent: "researcher".into(),
                source: "dag".into(),
                run_id: Some("run-1".into()),
                node_id: Some("node-1".into()),
                session_id: Some("sess-1".into()),
            },
            SubagentJobEvent::Output {
                id: "job-1".into(),
                chunk: "hi".into(),
                session_id: Some("sess-1".into()),
            },
            SubagentJobEvent::Metrics {
                id: "job-1".into(),
                tps: None,
                cps: None,
                chars: 0,
                tokens_in: 0,
                tokens_out: 0,
                tools_called: 0,
                turn: 0,
                session_id: Some("sess-1".into()),
            },
            SubagentJobEvent::Completed {
                id: "job-1".into(),
                status: SubagentJobStatus::Succeeded,
                error: None,
                chars: 0,
                tokens_in: 0,
                tokens_out: 0,
                tools_called: 0,
                session_id: Some("sess-1".into()),
            },
        ];

        for event in events {
            match agent_event(event) {
                WireAgentEvent::Started { session_id, .. }
                | WireAgentEvent::Output { session_id, .. }
                | WireAgentEvent::Metrics { session_id, .. }
                | WireAgentEvent::Completed { session_id, .. } => {
                    assert_eq!(session_id, "sess-1");
                }
            }
        }
    }

    #[test]
    fn agent_event_normalizes_missing_session_to_empty_string() {
        let event = SubagentJobEvent::Started {
            id: "job-1".into(),
            agent: "researcher".into(),
            source: "subagent".into(),
            run_id: None,
            node_id: None,
            session_id: None,
        };

        match agent_event(event) {
            WireAgentEvent::Started { session_id, .. } => assert_eq!(session_id, ""),
            other => panic!("expected Started event, got {other:?}"),
        }
    }

    #[test]
    fn dag_event_maps_session_ownership() {
        let run = DagEvent::RunStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            status: DagStatus::Running,
            error: None,
        };
        match dag_event(run) {
            WireDagEvent::RunStatus { session_id, .. } => assert_eq!(session_id, "sess-1"),
            other => panic!("expected RunStatus event, got {other:?}"),
        }

        let node = DagEvent::NodeStatus {
            run_id: "goal-1".into(),
            session_id: "sess-1".into(),
            node_id: "main".into(),
            status: NodeStatus::Running,
            error: None,
        };
        match dag_event(node) {
            WireDagEvent::NodeStatus { session_id, .. } => assert_eq!(session_id, "sess-1"),
            other => panic!("expected NodeStatus event, got {other:?}"),
        }
    }

    #[test]
    fn graph_ops_projects_lists_and_portable_checkpoints() {
        let source = Arc::new(DagEngine::new());
        let run_id = source.plan_goal("finish", Some("session-1".into()));
        let source_ops = CoreGraphOps::new(source);

        let runs = source_ops.list("session-1");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].id, run_id);
        assert_eq!(runs[0].kind, "goal");

        let checkpoints = source_ops.checkpoints("session-1", Some(&run_id)).unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].kind, WireGraphKind::Goal);

        let target = Arc::new(DagEngine::new());
        let target_ops = CoreGraphOps::new(target);
        let restored = target_ops
            .restore("session-2", &checkpoints[0].snapshot)
            .unwrap();
        assert_eq!(restored, run_id);
        assert_eq!(target_ops.list("session-2").len(), 1);
    }
}
