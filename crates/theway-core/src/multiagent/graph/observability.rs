use crate::observability::{
    ErrorCategory, ObservationContext, OperationDetail, OperationId, OperationOutcome,
    OperationScope, RuntimeMeasurements, RuntimeObserver,
};

use super::engine::DagEngine;
use super::types::{DagStatus, NodeStatus};

impl DagEngine {
    pub fn observer(&self) -> std::sync::Arc<dyn RuntimeObserver> {
        std::sync::Arc::clone(&self.observer)
    }

    pub(super) fn begin_run_observation(&self, run_id: &str) {
        let Some(run) = self.get_run(run_id) else {
            return;
        };
        let mut operations = self.run_operations.lock();
        if operations.contains_key(run_id) {
            return;
        }
        let scope = OperationScope::start(
            self.observer(),
            None,
            ObservationContext {
                session_id: run.session_id,
                run_id: Some(run_id.to_string()),
                ..ObservationContext::default()
            },
            OperationDetail::DagRun,
        );
        operations.insert(run_id.to_string(), scope);
    }

    pub fn run_operation_id(&self, run_id: &str) -> Option<OperationId> {
        self.run_operations
            .lock()
            .get(run_id)
            .map(OperationScope::id)
    }

    pub(super) fn begin_node_observation(&self, run_id: &str, node_id: &str) {
        let key = (run_id.to_string(), node_id.to_string());
        let Some(run) = self.get_run(run_id) else {
            return;
        };
        if run.node(node_id).is_none() {
            return;
        }
        self.begin_run_observation(run_id);
        let parent_id = self.run_operation_id(run_id);
        let mut operations = self.node_operations.lock();
        if operations.contains_key(&key) {
            return;
        }
        let scope = OperationScope::start(
            self.observer(),
            parent_id,
            ObservationContext {
                session_id: run.session_id,
                run_id: Some(run_id.to_string()),
                node_id: Some(node_id.to_string()),
                ..ObservationContext::default()
            },
            OperationDetail::DagNode,
        );
        operations.insert(key, scope);
    }

    pub fn node_operation_id(&self, run_id: &str, node_id: &str) -> Option<OperationId> {
        self.node_operations
            .lock()
            .get(&(run_id.to_string(), node_id.to_string()))
            .map(OperationScope::id)
    }

    pub(super) fn finish_node_observation(&self, run_id: &str, node_id: &str, status: NodeStatus) {
        let Some(scope) = self
            .node_operations
            .lock()
            .remove(&(run_id.to_string(), node_id.to_string()))
        else {
            return;
        };
        let node = self
            .get_run(run_id)
            .and_then(|run| run.node(node_id).cloned());
        let timed_out = node.as_ref().is_some_and(|node| {
            node.error.as_deref().is_some_and(|message| {
                let message = message.to_ascii_lowercase();
                message.contains("timed out") || message.contains("timeout")
            })
        });
        let measurements = node
            .map(|node| RuntimeMeasurements {
                input_tokens: node.input_tokens.unwrap_or_default(),
                output_tokens: node.output_tokens.unwrap_or_default(),
                characters: node
                    .output
                    .as_deref()
                    .map(|output| output.chars().count() as u64)
                    .unwrap_or_default(),
                ..RuntimeMeasurements::default()
            })
            .unwrap_or_default();
        let (outcome, category) = match status {
            NodeStatus::Succeeded => (OperationOutcome::Succeeded, None),
            NodeStatus::Failed if timed_out => {
                (OperationOutcome::TimedOut, Some(ErrorCategory::Timeout))
            }
            NodeStatus::Failed => (OperationOutcome::Failed, Some(ErrorCategory::Runtime)),
            NodeStatus::Skipped => (OperationOutcome::Skipped, None),
            NodeStatus::Cancelled => (
                OperationOutcome::Cancelled,
                Some(ErrorCategory::Cancellation),
            ),
            NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Running => {
                (OperationOutcome::Abandoned, Some(ErrorCategory::Runtime))
            }
        };
        scope.finish(outcome, category, measurements);
    }

    pub(super) fn finish_run_observation(&self, run_id: &str, status: DagStatus) {
        let Some(scope) = self.run_operations.lock().remove(run_id) else {
            return;
        };
        let measurements = self
            .get_run(run_id)
            .map(|run| RuntimeMeasurements {
                input_tokens: run
                    .nodes
                    .iter()
                    .map(|node| node.input_tokens.unwrap_or_default())
                    .sum(),
                output_tokens: run
                    .nodes
                    .iter()
                    .map(|node| node.output_tokens.unwrap_or_default())
                    .sum(),
                ..RuntimeMeasurements::default()
            })
            .unwrap_or_default();
        let (outcome, category) = match status {
            DagStatus::Completed => (OperationOutcome::Succeeded, None),
            DagStatus::Failed => (OperationOutcome::Failed, Some(ErrorCategory::Runtime)),
            DagStatus::Cancelled => (
                OperationOutcome::Cancelled,
                Some(ErrorCategory::Cancellation),
            ),
            DagStatus::Running => (OperationOutcome::Abandoned, Some(ErrorCategory::Runtime)),
        };
        scope.finish(outcome, category, measurements);
    }
}
