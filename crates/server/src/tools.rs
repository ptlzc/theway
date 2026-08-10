//! Tool ASSEMBLY layer. The engine (`theway_core`) supplies the harness-runtime tools
//! (graph/DAG, subagents, skills, memory, MCP — see `theway_core::tools`) AND assembles
//! them via `theway_core::tools::assembly` (`engine_tools` for the main agent,
//! `subagent_tools` for subagents); this module is the application-layer half:
//!
//! - **Local-execution tool bodies** (bash / shell / fs / git / grep / find / ls /
//!   outline / truncate) live HERE, not in the engine: they are environment-specific
//!   agent capabilities — the local ones may become remote sandbox execution later —
//!   so the engine must not depend on them.
//! - **Web tools** (`web_fetch` / `web_search`) are app-layer capabilities too (they
//!   need external credentials/configuration).
//! - **Assembly**: [`local_tools`] / the subagent tool-set resolver / `session_tool_set`
//!   wire engine tools + local tools together. The engine-owned part is assembled
//!   core-side ([`theway_core::tools::assembly`]); this module appends the
//!   local-execution tools and the server-side trigger/cron family.
//!
//! Subagent tool sets are injected into the engine (specs carry no tool
//! factory): [`subagent_tool_sets`] builds the ONE resolver both `subagent_tool` and the
//! DAG node launcher are constructed with — every spec gets the same uniform set (engine
//! tools minus `subagent`/`dag_*` plus local tools); behavior is prompt-defined.

use std::path::PathBuf;
use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::runtime::subagents::node_launcher;
use theway_core::runtime::subagents::node_launcher::ToolSetResolver;
use theway_core::runtime::subagents::registry::SubagentJobRegistry;
use theway_core::tools::skill::SkillHarnessCell;
use theway_core::tools::subagent;

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

/// Local-execution tool set (the app-layer half of the session tool set): shell / fs /
/// git / grep / web — everything that depends on the execution environment. No engine
/// tools here (DAG / subagent / skills / memory come from the engine's own assembly,
/// [`theway_core::tools::assembly`]); no duplicate-memory risk.
pub fn local_tools() -> Vec<Arc<dyn AgentTool>> {
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
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
    session_id: Option<String>,
) -> Arc<dyn AgentTool> {
    Arc::new(
        subagent::SubagentTool::new(
            model,
            stream_fn,
            subagent_tool_sets(memory_dir, skill_harness_cell),
            crate::subagent_specs::launch_resolver(),
            crate::subagent_specs::spec_names(),
            registry,
        )
        .with_session_id(session_id),
    )
}

/// Build the app-layer tool-set resolver injected into the subagent tool and the DAG
/// node launcher. ONE uniform set for every spec: engine tools minus the two
/// orchestration tools (`subagent` / `dag_*`) plus local tools — assembled core-side,
/// [`theway_core::tools::assembly::subagent_tools`]. Per-spec tool differences are gone;
/// behavior is defined by the spec's system prompt and the parent's task prompt.
pub fn subagent_tool_sets(
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
) -> ToolSetResolver {
    theway_core::tools::assembly::subagent_tools(
        &memory_dir,
        &skill_harness_cell,
        Arc::new(local_tools),
    )
}

/// Build a DAG node launcher wired to `engine`, with the app-layer tool-set resolver.
pub fn node_launcher(
    engine: Arc<DagEngine>,
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    cwd: PathBuf,
    registry: SubagentJobRegistry,
    memory_dir: PathBuf,
    skill_harness_cell: SkillHarnessCell,
) -> Arc<node_launcher::NodeLauncherImpl> {
    node_launcher::node_launcher(
        engine,
        model,
        stream_fn,
        cwd,
        registry,
        subagent_tool_sets(memory_dir, skill_harness_cell),
        crate::subagent_specs::launch_resolver(),
    )
}

/// Build the per-session tool set (session-resource-model). One source of truth shared by
/// the CLI's initial harness build and the session factory ([`crate::session_ops::SessionFactory`]):
/// everything here is either session-stamped (`dag_*` / `subagent`) or must be rebuilt per
/// harness (the skill family wires a fresh harness cell per build). Local tools
/// ([`local_tools`]) + engine tools ([`theway_core::tools::assembly::engine_tools`]) +
/// the server-side trigger/cron family. Process-level tool groups (MCP tools) are the
/// caller's to add.
pub fn session_tool_set(
    memory_dir: &std::path::Path,
    dag_engine: &Arc<DagEngine>,
    subagent_registry: &SubagentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &SkillHarnessCell,
    session_id: &str,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = local_tools();
    // Engine-owned tools (DAG / subagent / skills / memory), assembled core-side with the
    // same subagent tool-set resolver the DAG node launcher uses.
    tools.extend(theway_core::tools::assembly::engine_tools(
        memory_dir,
        dag_engine,
        subagent_registry,
        subagent_tool_sets(memory_dir.to_path_buf(), skill_harness_cell.clone()),
        crate::subagent_specs::launch_resolver(),
        crate::subagent_specs::spec_names(),
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
