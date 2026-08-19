//! Subagent data contract — the types shared between the engine and the app.
//!
//! The engine provides the subagent CAPABILITY (shared runner, DAG node launcher,
//! `subagent` tool); it does not define what a "spec" is. The spec concept — its
//! structure, the table of built-in specs, their system prompts — is app-layer
//! content (the daemon kernel, `theway_daemon::agent_specs`). The only thing the
//! engine needs at launch time is the resolved launch parameters: name, description,
//! system prompt, iteration budget. The app resolves its spec table into these via
//! the injected [`AgentRunResolver`] (same pattern as the tool-set resolver).

use std::sync::Arc;

use theway_core::AgentTool;

/// Launch parameters for one subagent run — pure data, resolved app-side from the
/// app's spec table.
#[derive(Clone, Copy)]
pub struct AgentRunParams {
    pub name: &'static str,
    pub description: &'static str,
    /// Short (1–3 sentences) system prompt.
    pub system_prompt: &'static str,
    /// Iteration budget applied to the subagent harness for this run.
    pub max_iterations: u32,
}

/// App-layer launch resolver: spec name -> launch parameters. Injected at construction
/// (same pattern as the tool-set resolver); the app owns the spec table and how its
/// specs map to launch parameters.
pub type AgentRunResolver = Arc<dyn Fn(&str) -> Option<AgentRunParams> + Send + Sync>;

/// App-layer tool-set resolver: spec name -> tool set for the sub-harness. Injected at
/// construction (same pattern as [`AgentRunResolver`]); the app owns which tools each spec
/// gets. The engine does not know which tools exist; the daemon owns local tool construction
/// and may resolve tools to remote sandbox execution.
pub type ToolSetResolver = Arc<dyn Fn(&str) -> Vec<Arc<dyn AgentTool>> + Send + Sync>;
