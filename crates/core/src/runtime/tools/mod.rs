//! Built-in tools. Modeled on `packages/coding-agent/src/core/tools/` (the TS implementation):
//! same names, same parameter shapes, simpler bodies. Each tool implements
//! [`theway_core::AgentTool`].
//!
//! Split (openspec tools-into-core): the tool BODIES live here in the engine crate; the
//! injection-style ASSEMBLY (session-stamped / harness-cell-wired constructors like
//! `session_tool_set`, `task_tool`, the skill family, cron/trigger builders) stays in the
//! application layer (`theway` crate, `src/tools.rs`). The two set constructors below stay
//! here because engine code (`subagent_specs`' built-in tool-set factories) consumes them
//! directly and they need no runtime injection.

pub mod bash;
pub mod dag_tools;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod install_skill;
pub mod ls;
pub mod mcp_adapter;
pub mod memory;
pub mod node_launcher;
pub mod outline;
pub mod read;
pub mod remove_skill;
pub mod set_skill_state;
pub mod shell;
pub mod skill;
pub mod skill_builder;
pub mod subagent_runner;
pub mod subagent_specs;
pub mod task;
pub mod truncate;
pub mod web_fetch;
pub mod web_search;
pub mod write;

use std::sync::Arc;

use theway_core::AgentTool;

/// Default tool set the coding agent ships with. Order matches the TS `createCodingTools()`
/// + the read-only quartet (`grep`/`find`/`ls`) the TS exposes via `createAllTools()`.
pub fn default_tools(memory_dir: std::path::PathBuf) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(write::WriteTool),
        Arc::new(edit::EditTool),
        Arc::new(bash::BashTool),
        Arc::new(shell::ExecTool),
        Arc::new(shell::GetOutputTool),
        Arc::new(shell::KillShellTool),
        Arc::new(shell::WriteToProcessTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(web_search::WebSearchTool::new()),
        Arc::new(git::GitTool),
        Arc::new(memory::MemoryTool::new(memory_dir)),
    ]
}

/// Read-only tool set used by spawned subagents (issue #11). No `write`/`edit`/`bash` — a
/// subagent should not mutate the workspace; if it needs to, the parent agent should run the
/// write itself.
pub fn subagent_read_only_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(git::GitTool),
    ]
}
