//! High-frequency event plane types for the job registry (graph mode).

/// High-frequency event plane (graph mode): broadcast by the registry as jobs
/// start, produce output, update metrics, and complete. Transport-agnostic — the
/// transport layer converts these into the wire `StreamEvent` (see
/// `proto/theway_grpc.proto`).
#[derive(Clone, Debug)]
pub enum AgentJobEvent {
    Started {
        id: String,
        agent: String,
        source: String,
        run_id: Option<String>,
        node_id: Option<String>,
    },
    Output {
        id: String,
        chunk: String,
    },
    Metrics {
        id: String,
        tps: Option<f64>,
        cps: Option<f64>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
        turn: u32,
    },
    Completed {
        id: String,
        status: JobStatus,
        error: Option<String>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// The current turn was interrupted (`AgentControlHandle::interrupt`) and no
    /// steering was queued, so the run ended at the turn boundary.
    Interrupted,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Succeeded => "succeeded",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Interrupted => "interrupted",
        }
    }
}
