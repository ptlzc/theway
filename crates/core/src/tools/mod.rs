//! Core capability tools — agent-facing generic tools (process/files/network/
//! search/git/MCP) that do not support the harness runtime itself; they are
//! plain capabilities the agent can call. Harness-supporting tools (subagents,
//! DAG orchestration, skills, memory) live in `crate::runtime::tools`.

pub mod bash;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod ls;
pub mod mcp_adapter;
pub mod outline;
pub mod read;
pub mod shell;
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
        Arc::new(crate::tools::read::ReadTool),
        Arc::new(crate::tools::write::WriteTool),
        Arc::new(crate::tools::edit::EditTool),
        Arc::new(crate::tools::bash::BashTool),
        Arc::new(crate::tools::shell::ExecTool),
        Arc::new(crate::tools::shell::GetOutputTool),
        Arc::new(crate::tools::shell::KillShellTool),
        Arc::new(crate::tools::shell::WriteToProcessTool),
        Arc::new(crate::tools::ls::LsTool),
        Arc::new(crate::tools::grep::GrepTool),
        Arc::new(crate::tools::find::FindTool),
        Arc::new(crate::tools::web_fetch::WebFetchTool),
        Arc::new(crate::tools::web_search::WebSearchTool::new()),
        Arc::new(crate::tools::git::GitTool),
        Arc::new(crate::runtime::tools::memory::MemoryTool::new(memory_dir)),
    ]
}

/// Read-only tool set used by spawned subagents (issue #11). No `write`/`edit`/`bash` — a
/// subagent should not mutate the workspace; if it needs to, the parent agent should run the
/// write itself.
pub fn subagent_read_only_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(crate::tools::read::ReadTool),
        Arc::new(crate::tools::ls::LsTool),
        Arc::new(crate::tools::grep::GrepTool),
        Arc::new(crate::tools::find::FindTool),
        Arc::new(crate::tools::web_fetch::WebFetchTool),
        Arc::new(crate::tools::git::GitTool),
    ]
}
