//! `subagent_launch` — the launch DATA channel between the engine and the app.
//!
//! The engine provides the subagent CAPABILITY (shared runner, DAG node launcher,
//! `subagent` tool); it does not define what a "spec" is. The spec concept — its
//! structure, the table of built-in specs, their system prompts — is app-layer
//! content (the `theway` server crate, `crate::subagent_specs`). The only thing the
//! engine needs at launch time is the resolved launch parameters: name, description,
//! system prompt, iteration budget. The app resolves its spec table into these via
//! the injected [`LaunchResolver`] (same pattern as the tool-set resolver).

use std::sync::Arc;

/// Launch parameters for one subagent run — pure data, resolved app-side from the
/// app's spec table.
#[derive(Clone, Copy)]
pub struct SubagentLaunch {
    pub name: &'static str,
    pub description: &'static str,
    /// Short (1–3 sentences) system prompt.
    pub system_prompt: &'static str,
    /// Iteration budget. The harness has no hard per-run cap today (the agent loop runs
    /// until the model stops); the field mirrors the documented budget and is reserved
    /// for future enforcement.
    pub max_iterations: u32,
}

/// App-layer launch resolver: spec name -> launch parameters. Injected at construction
/// (same pattern as the tool-set resolver); the app owns the spec table and how its
/// specs map to launch parameters.
pub type LaunchResolver = Arc<dyn Fn(&str) -> Option<SubagentLaunch> + Send + Sync>;
