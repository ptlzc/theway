//! Tool ASSEMBLY layer. The engine (`theway_core`) supplies the harness-runtime tools
//! (graph/DAG, subagents, skills, memory, MCP — see `theway_core::tools`) AND assembles
//! them via `theway_core::tools::assembly::session_engine_tools`; this module is the
//! application-layer half:
//!
//! - **Local-execution tool bodies** (bash / shell / fs / git / grep / find / ls /
//!   outline / truncate) live HERE, not in the engine: they are environment-specific
//!   agent capabilities — the local ones may become remote sandbox execution later —
//!   so the engine must not depend on them.
//! - **Web tools** (`web_fetch` / `web_search`) are app-layer capabilities too (they
//!   need external credentials/configuration).
//! - **Assembly**: `default_tools` / `subagent_read_only_tools` / the per-subagent
//!   tool-set resolver / `session_tool_set` wire engine tools + local tools together.
//!   The engine-owned part (DAG / task / skills / memory) is assembled core-side
//!   ([`theway_core::tools::assembly::session_engine_tools`]); this module appends the
//!   local-execution tools and the server-side trigger/cron family.
//!
//! Subagent tool sets are injected into the engine (`SubagentSpec` carries no tool
//! factory): [`subagent_tool_sets`] builds the ONE resolver both `task_tool` and the
//! DAG node launcher are constructed with — task and DAG share one subagent mechanism.

use std::path::PathBuf;
use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::runtime::subagents::registry::SubagentJobRegistry;
use theway_core::tools::node_launcher::ToolSetResolver;
use theway_core::tools::skill::SkillHarnessCell;
use theway_core::tools::{node_launcher, task};

// ── tool bodies (app-layer: local execution + web) ──────────────────────────
//
// Plain module declarations: `tests/tools.rs` pulls in only `triggers/tool_assembly.rs`
// by `#[path]`, so nothing here needs `#[path]` re-routing — a plain `pub mod shell;`
// resolves `src/tools/shell.rs` and lets `shell.rs`'s own `mod tests;` find
// `src/tools/shell/tests/` (a `#[path]`-included module would break that resolution).

pub mod bash;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod ls;
pub mod outline;
pub mod read;
pub mod shell;
pub mod truncate;
pub mod web_fetch;
pub mod web_search;
pub mod write;

// Trigger/cron model-facing constructors live in `triggers::tool_assembly` (kept
// out of this file so `tests/tools.rs` can `#[path]`-include just those).
pub use crate::triggers::tool_assembly::{
    list_cron_jobs_tool, list_triggers_tool, new_cron_job_tool, new_trigger_tool,
    remove_cron_job_tool, remove_trigger_tool, set_cron_job_state_tool, set_trigger_state_tool,
};

/// Default tool set the main coding agent ships with: local-execution tools, web tools,
/// and the engine's memory tool. This is the app-layer composition — the engine itself
/// (`theway_core::tools`) has no opinion about which local tools exist.
pub fn default_tools(memory_dir: PathBuf) -> Vec<Arc<dyn AgentTool>> {
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
        Arc::new(outline::OutlineTool),
        Arc::new(git::GitTool),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(web_search::WebSearchTool::new()),
        Arc::new(theway_core::tools::memory::MemoryTool::new(memory_dir)),
    ]
}

/// Read-only tool set for the read-only subagent specs (explorer / general). No
/// `write`/`edit`/`bash` — these specs are research agents; if the work needs mutation,
/// the parent agent should run the write itself. No web tools either (research is
/// intentionally scoped to local reads).
///
/// NOTE: the `task` tool's subagent is NOT bound to this set anymore — it ships the
/// full [`default_tools`] (same as the DAG `executor-coder`), so the main agent defines
/// in the task prompt what the subagent may do (mutate, run commands, etc.).
pub fn subagent_read_only_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(git::GitTool),
    ]
}

/// Build the `Task` tool. Separate from `default_tools` because Task needs the model handle to
/// spawn its inner harness; the caller wires it in at construction time. `session_id`
/// (session-resource-model) stamps the owning session on every spawned job — each harness
/// build gets its own TaskTool stamped with that harness's session.
///
/// The subagent tool set comes from [`subagent_tool_sets`] — the SAME resolver the DAG
/// node launcher gets, so `task` and DAG subagents share one mechanism. The main agent
/// picks the spec (explorer / planner / executor-coder / checker / general) and defines
/// in the prompt what the subagent may do.
pub fn task_tool(
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
    session_id: Option<String>,
) -> Arc<dyn AgentTool> {
    Arc::new(
        task::TaskTool::new(model, stream_fn, subagent_tool_sets(memory_dir), registry)
            .with_session_id(session_id),
    )
}

/// Subagent tool-set factories, one per built-in spec name (mirrors the old engine-side
/// factories; they moved here because they compose LOCAL tools). The executor-coder
/// shares the parent's memory dir so DAG subagents use the same store as the parent.
fn explorer_tools() -> Vec<Arc<dyn AgentTool>> {
    subagent_read_only_tools()
}

fn planner_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
    ]
}

fn executor_coder_tools(memory_dir: PathBuf) -> Vec<Arc<dyn AgentTool>> {
    default_tools(memory_dir)
}

fn checker_tools() -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(bash::BashTool),
        Arc::new(git::GitTool),
    ]
}

fn general_tools() -> Vec<Arc<dyn AgentTool>> {
    subagent_read_only_tools()
}

/// Build the app-layer tool-set resolver injected into the DAG node launcher: spec name
/// → tool set. Unknown names yield an empty set (the engine rejects unknown agents via
/// `resolve_spec` before the resolver is ever consulted).
pub fn subagent_tool_sets(memory_dir: PathBuf) -> ToolSetResolver {
    Arc::new(move |name| match name {
        "explorer" => explorer_tools(),
        "planner" => planner_tools(),
        "executor-coder" => executor_coder_tools(memory_dir.clone()),
        "checker" => checker_tools(),
        "general" => general_tools(),
        _ => Vec::new(),
    })
}

/// Build a DAG node launcher wired to `engine`, with the app-layer tool-set resolver.
pub fn node_launcher(
    engine: Arc<DagEngine>,
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    cwd: PathBuf,
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
) -> Arc<node_launcher::NodeLauncherImpl> {
    node_launcher::node_launcher(
        engine,
        model,
        stream_fn,
        cwd,
        registry,
        subagent_tool_sets(memory_dir),
    )
}

/// Build the per-session tool set (session-resource-model). One source of truth shared by
/// the CLI's initial harness build and the session factory ([`crate::session_ops::SessionFactory`]):
/// everything here is either session-stamped (`dag_*` / `task`) or must be rebuilt per
/// harness (the skill family wires a fresh harness cell per build). Engine-owned tools
/// are assembled core-side; this function appends the local-execution tools
/// ([`default_tools`]) and the server-side trigger/cron family. Process-level tool
/// groups (MCP tools) are the caller's to add.
pub fn session_tool_set(
    memory_dir: &std::path::Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = default_tools(memory_dir.to_path_buf());
    // Engine-owned tools (DAG / task / skills / memory), assembled core-side with the
    // same subagent tool-set resolver the DAG node launcher uses.
    tools.extend(theway_core::tools::assembly::session_engine_tools(
        memory_dir,
        dag_engine,
        subagent_registry,
        subagent_tool_sets(memory_dir.to_path_buf()),
        model,
        stream_fn,
        skill_harness_cell,
        session_id,
    ));
    // Trigger/cron family: harness-adjacent but implemented in this crate.
    tools.push(new_cron_job_tool(skill_harness_cell.clone()));
    tools.push(list_cron_jobs_tool());
    tools.push(remove_cron_job_tool(skill_harness_cell.clone()));
    tools.push(set_cron_job_state_tool(skill_harness_cell.clone()));
    tools.push(new_trigger_tool());
    tools.push(list_triggers_tool());
    tools.push(remove_trigger_tool());
    tools.push(set_trigger_state_tool());
    tools
}
