//! Transport-neutral runtime observations.
//!
//! Product events (`LoopEvent`, `SessionEvent`, `SubagentJobEvent`, and `DagEvent`) retain
//! their UI, persistence, and wire semantics. This module is a separate, content-safe port
//! for embedders that need traces, metrics, or structured operational logs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

/// Process-local identity for one observed operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OperationId(u64);

impl OperationId {
    fn next() -> Self {
        Self(NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Safe correlation values shared by runtime operations.
///
/// Identifiers are useful trace/log attributes but MUST NOT be used as metric labels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObservationContext {
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub turn_id: Option<u32>,
    pub job_id: Option<String>,
    pub node_id: Option<String>,
}

impl ObservationContext {
    pub fn with_turn(&self, turn_id: u32) -> Self {
        Self {
            turn_id: Some(turn_id),
            ..self.clone()
        }
    }

    pub fn with_graph(
        &self,
        run_id: Option<String>,
        node_id: Option<String>,
        job_id: Option<String>,
    ) -> Self {
        Self {
            run_id,
            node_id,
            job_id,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OperationKind {
    AgentRun,
    Turn,
    LlmRequest,
    ToolExecution,
    Compaction,
    SubagentJob,
    DagRun,
    DagNode,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentRun => "agent.run",
            Self::Turn => "agent.turn",
            Self::LlmRequest => "llm.request",
            Self::ToolExecution => "tool.execute",
            Self::Compaction => "session.compaction",
            Self::SubagentJob => "multiagent.job",
            Self::DagRun => "dag.run",
            Self::DagNode => "dag.node",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OperationOutcome {
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
    TimedOut,
    Skipped,
    Abandoned,
}

impl OperationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::TimedOut => "timed_out",
            Self::Skipped => "skipped",
            Self::Abandoned => "abandoned",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorCategory {
    Provider,
    Tool,
    Permission,
    Persistence,
    Validation,
    Timeout,
    Cancellation,
    Runtime,
}

impl ErrorCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Tool => "tool",
            Self::Permission => "permission",
            Self::Persistence => "persistence",
            Self::Validation => "validation",
            Self::Timeout => "timeout",
            Self::Cancellation => "cancellation",
            Self::Runtime => "runtime",
        }
    }
}

/// Neutral measurements. USD pricing and application budget policy are intentionally absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuntimeMeasurements {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub characters: u64,
    pub turns: u64,
    pub tool_calls: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationDetail {
    AgentRun,
    Turn {
        index: u32,
    },
    LlmRequest {
        provider: String,
        model: String,
    },
    ToolExecution {
        tool_name: String,
    },
    Compaction {
        algorithm: String,
        provider: String,
        model: String,
    },
    SubagentJob {
        agent: String,
        source: String,
    },
    DagRun,
    DagNode,
}

impl OperationDetail {
    pub fn kind(&self) -> OperationKind {
        match self {
            Self::AgentRun => OperationKind::AgentRun,
            Self::Turn { .. } => OperationKind::Turn,
            Self::LlmRequest { .. } => OperationKind::LlmRequest,
            Self::ToolExecution { .. } => OperationKind::ToolExecution,
            Self::Compaction { .. } => OperationKind::Compaction,
            Self::SubagentJob { .. } => OperationKind::SubagentJob,
            Self::DagRun => OperationKind::DagRun,
            Self::DagNode => OperationKind::DagNode,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationStarted {
    pub id: OperationId,
    pub parent_id: Option<OperationId>,
    pub context: ObservationContext,
    pub detail: OperationDetail,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationFinished {
    pub id: OperationId,
    pub kind: OperationKind,
    pub context: ObservationContext,
    pub outcome: OperationOutcome,
    pub error_category: Option<ErrorCategory>,
    pub duration: Duration,
    pub measurements: RuntimeMeasurements,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeObservation {
    OperationStarted(OperationStarted),
    OperationFinished(OperationFinished),
}

/// Embedder-owned, non-blocking observation port.
pub trait RuntimeObserver: Send + Sync {
    fn observe(&self, observation: RuntimeObservation);
}

#[derive(Debug, Default)]
pub struct NoopRuntimeObserver;

impl RuntimeObserver for NoopRuntimeObserver {
    fn observe(&self, _observation: RuntimeObservation) {}
}

pub fn noop_runtime_observer() -> Arc<dyn RuntimeObserver> {
    Arc::new(NoopRuntimeObserver)
}

/// Invoke an observer without allowing its panic to enter runtime control flow.
pub fn dispatch(observer: &Arc<dyn RuntimeObserver>, observation: RuntimeObservation) {
    let observer = Arc::clone(observer);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        observer.observe(observation);
    }));
}

/// RAII lifecycle helper. Dropping an unfinished scope emits `Abandoned` exactly once.
pub struct OperationScope {
    id: OperationId,
    kind: OperationKind,
    context: ObservationContext,
    observer: Arc<dyn RuntimeObserver>,
    started_at: Instant,
    finished: bool,
}

impl OperationScope {
    pub fn start(
        observer: Arc<dyn RuntimeObserver>,
        parent_id: Option<OperationId>,
        context: ObservationContext,
        detail: OperationDetail,
    ) -> Self {
        let id = OperationId::next();
        let kind = detail.kind();
        dispatch(
            &observer,
            RuntimeObservation::OperationStarted(OperationStarted {
                id,
                parent_id,
                context: context.clone(),
                detail,
            }),
        );
        Self {
            id,
            kind,
            context,
            observer,
            started_at: Instant::now(),
            finished: false,
        }
    }

    pub fn id(&self) -> OperationId {
        self.id
    }

    pub fn finish(
        mut self,
        outcome: OperationOutcome,
        error_category: Option<ErrorCategory>,
        measurements: RuntimeMeasurements,
    ) {
        self.emit_finish(outcome, error_category, measurements);
    }

    fn emit_finish(
        &mut self,
        outcome: OperationOutcome,
        error_category: Option<ErrorCategory>,
        measurements: RuntimeMeasurements,
    ) {
        if self.finished {
            return;
        }
        self.finished = true;
        dispatch(
            &self.observer,
            RuntimeObservation::OperationFinished(OperationFinished {
                id: self.id,
                kind: self.kind,
                context: self.context.clone(),
                outcome,
                error_category,
                duration: self.started_at.elapsed(),
                measurements,
            }),
        );
    }
}

impl Drop for OperationScope {
    fn drop(&mut self) {
        self.emit_finish(
            OperationOutcome::Abandoned,
            Some(ErrorCategory::Runtime),
            RuntimeMeasurements::default(),
        );
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("observability");
