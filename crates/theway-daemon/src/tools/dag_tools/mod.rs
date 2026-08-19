//! `dag_*` tools — the DAG orchestration tool face: define (dag_plan), monitor
//! (dag_status / dag_inspect), harvest (dag_wait), intervene (dag_retry /
//! dag_skip / dag_cancel). 1:1 port of the dag-orchestrator extension's
//! `tools.ts`, driving the engine in
//! `theway_core::multiagent::graph::engine` (which owns the
//! scheduler/state machine; the real subagent launcher lives in
//! `crate::multiagent::graph::node_launcher` and is wired in by the app layer).
//!
//! Session isolation: runs are stamped with the owning pi session id, and every
//! tool refuses runs owned by another session (multiple concurrent agents in
//! one project never cross-trigger each other's DAGs). The session id is
//! injected at construction by p3c-wire (`None` = no isolation, e.g. REPL).
//!
//! Layout: one submodule per tool ([`plan`], [`status`], [`inspect`], [`wait`],
//! [`retry`], [`skip`], [`cancel`]) plus the shared helpers in [`utils`].

use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;

// The test mirror (`tests/tools/dag_tools/`, bridged at the bottom of this
// file) resolves these names through this module's scope via `use super::*`;
// production code imports them in the tool submodules directly, so the
// re-imports here are test-only.
#[cfg(test)]
use serde_json::{Value, json};
#[cfg(test)]
use theway_core::AgentToolError;
#[cfg(test)]
use theway_core::multiagent::graph::types::{DagNodeDef, DagRunDef, NodeStatus};
#[cfg(test)]
use theway_llm_provider::UserContentBlock;
#[cfg(test)]
use tokio_util::sync::CancellationToken;
#[cfg(test)]
use utils::{
    civil_from_days, iso_time_ms, node_result_text, status_counts, tail_truncate, thousands,
};

pub mod cancel;
pub mod inspect;
pub mod plan;
pub mod retry;
pub mod skip;
pub mod status;
pub mod utils;
pub mod wait;

// Keep the pre-split public paths (`dag_tools::DagPlanTool`, …) reachable.
pub use cancel::DagCancelTool;
pub use inspect::DagInspectTool;
pub use plan::{DagPlanTool, plan_from_definition};
pub use retry::DagRetryTool;
pub use skip::DagSkipTool;
pub use status::DagStatusTool;
pub use wait::DagWaitTool;

// ── constants ────────────────────────────────────────────────────────────────

const DAG_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 120;
const DAG_WAIT_IDLE_SECS: u64 = 30;
const NODE_RESULT_DEFAULT_TAIL: usize = 800;

// ── construction ─────────────────────────────────────────────────────────────

/// Build the seven `dag_*` tools, all sharing one engine and the owning pi
/// session id (p3c-wire passes `Some(session_id)` from the harness; `None`
/// disables session isolation).
pub struct DagTools;

impl DagTools {
    /// Returns the tool vec rather than Self by contract — p3c-wire calls this to
    /// build the dag_* tool set for the binary.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        engine: Arc<DagEngine>,
        session_id: Option<String>,
        spec_names: Vec<String>,
        registry: SubagentJobRegistry,
    ) -> Vec<Arc<dyn AgentTool>> {
        vec![
            Arc::new(DagPlanTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
                spec_names: spec_names.clone(),
            }),
            Arc::new(DagStatusTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagInspectTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
                registry: registry.clone(),
            }),
            Arc::new(DagWaitTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagRetryTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagSkipTool {
                engine: engine.clone(),
                session_id: session_id.clone(),
            }),
            Arc::new(DagCancelTool { engine, session_id }),
        ]
    }
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
tests_bridge_macro::tests_bridge!("tools/dag_tools");
