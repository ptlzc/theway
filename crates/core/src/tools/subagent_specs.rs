//! `subagent_specs` — the spec MECHANISM: the [`SubagentSpec`] structure and the
//! [`SpecResolver`] injection point.
//!
//! The engine defines WHAT a spec looks like and HOW it is resolved, but not WHICH
//! specs exist. The actual spec table (explorer / planner / executor-coder / checker /
//! general) is an app-layer decision — the behavior content lives in the `theway`
//! server crate (`crate::subagent_specs`), same line as the tool-set resolver:
//! orchestration mechanisms live in the engine, behavior lives in the app.
//!
//! [`node_launcher`](super::node_launcher) and the `subagent` tool resolve a node's /
//! call's `agent` field through the injected resolver; unknown names fail with
//! `unknown agent "..."` (mirrors the TS `defaultLauncher`).

use std::sync::Arc;

/// Static description of a built-in subagent.
#[derive(Clone, Copy)]
pub struct SubagentSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// Short (1–3 sentences) system prompt, same style as the `subagent` tool's.
    pub system_prompt: &'static str,
    /// Iteration budget. The harness has no hard per-run cap today (the agent loop runs
    /// until the model stops); the field mirrors the `subagent` tool's documented budget and
    /// is reserved for future enforcement.
    pub max_iterations: u32,
}

/// Default iteration budget, matching the `subagent` tool's "max 16 iterations" doc.
pub const DEFAULT_MAX_ITERATIONS: u32 = 16;

/// App-layer spec lookup: spec name -> spec. Injected at construction (same pattern as
/// the tool-set resolver); the app owns the spec table and its behavior content.
pub type SpecResolver = Arc<dyn Fn(&str) -> Option<SubagentSpec> + Send + Sync>;
