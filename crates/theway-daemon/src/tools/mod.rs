//! Tool ASSEMBLY layer — the application-layer half of the session tool set.
//!
//! All tool BODIES (harness-runtime tools AND local-execution tools) live HERE, in
//! the daemon (daemon-kernel-layers: the daemon is the single kernel; the engine
//! crate `theway_core` keeps only the `AgentTool` / `ToolExecutor` / `ExecutionEnv`
//! trait families and the agent runtime):
//!
//! - **Harness-runtime tools** (graph/DAG, subagents, skills, memory, MCP — formerly
//!   `theway_core::tools`) sit next to the local bodies: `assembly` / `subagent` /
//!   `dag_tools` / the skill family / `memory` / `mcp_adapter` / `exec_shell`.
//! - **Local-execution tool bodies** (bash / fs / git / grep / find / ls / outline /
//!   truncate) are environment-specific agent capabilities.
//! - **Web tools** (`web_fetch` / `web_search`) are app-layer capabilities too (they
//!   need external credentials/configuration).
//! - **Assembly**: [`local_tools`] / the subagent tool-set resolver / `session_tool_set`
//!   wire engine tools + local tools together, then append the server-side
//!   trigger/cron family.
//!
//! Subagent tool sets are injected into the engine (specs carry no tool
//! factory): [`subagent_tool_sets`] builds the ONE resolver both `subagent_tool` and the
//! DAG node launcher are constructed with — every spec gets the same uniform set (engine
//! tools minus `subagent`/`dag_*` plus local tools); behavior is prompt-defined.

use std::path::PathBuf;
use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::executor::ToolExecutor;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::node_launcher;
use theway_core::multiagent::registry::AgentJobRegistry;
use theway_core::multiagent::types::ToolSetResolver;

use crate::tools::skill::SkillHarnessCell;

// ── tool bodies (harness-runtime + app-layer) ───────────────────────────────
//
// Plain module declarations: `tests/tools.rs` pulls in only `triggers/tool_assembly.rs`
// by `#[path]`, so nothing here needs `#[path]` re-routing — a plain `pub mod bash;`
// resolves `src/tools/bash.rs` and lets `bash.rs`'s own `mod tests;` find
// `src/tools/bash/tests/` (a `#[path]`-included module would break that resolution).

// Harness-runtime tools (moved from theway-core, daemon-kernel-layers).
pub mod assembly;
pub mod dag_tools;
pub mod exec;
pub mod exec_shell;
pub mod install_skill;
pub mod mcp_adapter;
pub mod memory;
pub mod remove_skill;
pub mod set_skill_state;
pub mod skill;
pub mod skill_builder;
pub mod subagent;

// App-layer local-execution + web tools.
pub mod bash;
pub mod edit;
pub mod find;
pub mod git;
pub mod grep;
pub mod ls;
pub mod outline;
pub mod read;
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

/// Local-execution tool set (the app-layer half of the session tool set): shell / fs /
/// git / grep / web — everything that depends on the execution environment. No engine
/// tools here (DAG / subagent / skills / memory come from the kernel's own assembly,
/// [`crate::tools::assembly`]); no duplicate-memory risk.
///
/// Executor binding (sdk-split-local-sandbox node 8): file-content and process tools
/// (read / write / edit / outline / git) dispatch their effects through the injected
/// [`ToolExecutor`] — the local executor for local editing mode; a sandbox
/// executor swaps the execution environment without touching tool definitions. The
/// remaining local tools are not yet wired through the executor: `bash` keeps its
/// process-group-kill + cancel semantics (the executor's `run_command` kills only the
/// direct child), and `ls` / `grep` / `find` / the `exec_shell` family use richer
/// directory/walk surfaces than the executor trait's first cut exposes.
pub fn local_tools(executor: Arc<dyn ToolExecutor>) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool::new(executor.clone())),
        Arc::new(write::WriteTool::new(executor.clone())),
        Arc::new(edit::EditTool::new(executor.clone())),
        Arc::new(bash::BashTool),
        Arc::new(exec_shell::ExecTool),
        Arc::new(exec_shell::GetOutputTool),
        Arc::new(exec_shell::KillShellTool),
        Arc::new(exec_shell::WriteToProcessTool),
        Arc::new(ls::LsTool),
        Arc::new(grep::GrepTool),
        Arc::new(find::FindTool),
        Arc::new(outline::OutlineTool::new(executor.clone())),
        Arc::new(git::GitTool::new(executor)),
        Arc::new(web_fetch::WebFetchTool),
        Arc::new(web_search::WebSearchTool::new()),
    ]
}

/// Build the `Subagent` tool. Separate from `local_tools` because the tool needs the model handle to
/// spawn its inner harness; the caller wires it in at construction time. `session_id`
/// (session-resource-model) stamps the owning session on every spawned job — each harness
/// build gets its own SubagentTool stamped with that harness's session.
///
/// The subagent tool set comes from [`subagent_tool_sets`] — the SAME resolver the DAG
/// node launcher gets (one uniform set per spec: engine tools minus subagent/dag_* plus
/// local tools). The main agent picks the spec (explorer / planner / executor-coder /
/// checker / general) and defines in the prompt what the subagent may do.
pub fn subagent_tool(
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    registry: AgentJobRegistry,
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    session_id: Option<String>,
    executor: Arc<dyn ToolExecutor>,
) -> Arc<dyn AgentTool> {
    Arc::new(
        subagent::SubagentTool::new(
            model,
            stream_fn,
            subagent_tool_sets(memory_dir, skill_harness_cell, executor),
            crate::agent_specs::launch_resolver(),
            crate::agent_specs::spec_names(),
            registry,
        )
        .with_session_id(session_id),
    )
}

/// Build the app-layer tool-set resolver injected into the subagent tool and the DAG
/// node launcher. ONE uniform set for every spec: engine tools minus the two
/// orchestration tools (`subagent` / `dag_*`) plus local tools — assembled kernel-side,
/// [`crate::tools::assembly::subagent_tools`]. Per-spec tool differences are gone;
/// behavior is defined by the spec's system prompt and the parent's task prompt.
pub fn subagent_tool_sets(
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    executor: Arc<dyn ToolExecutor>,
) -> ToolSetResolver {
    assembly::subagent_tools(
        &memory_dir,
        &skill_harness_cell,
        // The kernel-side local-tools factory closes over the daemon's executor, so every
        // subagent / DAG-node tool set dispatches through the same execution environment.
        Arc::new(move || local_tools(executor.clone())),
    )
}

/// Build a DAG node launcher wired to `engine`, with the app-layer tool-set resolver.
pub fn node_launcher(
    engine: Arc<DagEngine>,
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    cwd: PathBuf,
    registry: AgentJobRegistry,
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    executor: Arc<dyn ToolExecutor>,
) -> Arc<node_launcher::NodeLauncherImpl> {
    node_launcher::node_launcher(
        engine,
        model,
        stream_fn,
        cwd,
        registry,
        subagent_tool_sets(memory_dir, skill_harness_cell, executor),
        crate::agent_specs::launch_resolver(),
    )
}

/// Build the per-session tool set (session-resource-model). One source of truth shared by
/// the CLI's initial harness build and the session factory ([`crate::session_ops::SessionFactory`]):
/// everything here is either session-stamped (`dag_*` / `subagent`) or must be rebuilt per
/// harness (the skill family wires a fresh harness cell per build). Local tools
/// ([`local_tools`]) + engine tools ([`crate::tools::assembly::engine_tools`]) +
/// the server-side trigger/cron family. Process-level tool groups (MCP tools) are the
/// caller's to add.
pub fn session_tool_set(
    memory_dir: &std::path::Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &AgentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
    executor: Arc<dyn ToolExecutor>,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = local_tools(executor.clone());
    // Engine-owned tools (DAG / subagent / skills / memory), assembled kernel-side with the
    // same subagent tool-set resolver the DAG node launcher uses.
    tools.extend(assembly::engine_tools(
        memory_dir,
        dag_engine,
        subagent_registry,
        subagent_tool_sets(
            memory_dir.to_path_buf(),
            skill_harness_cell.clone(),
            executor,
        ),
        crate::agent_specs::launch_resolver(),
        crate::agent_specs::spec_names(),
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
