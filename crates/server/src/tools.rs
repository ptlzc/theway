//! Tool ASSEMBLY layer. The engine (`theway_core`) supplies the harness-runtime tools
//! (graph/DAG, subagents, skills, memory, MCP — see `theway_core::tools`); this module
//! is the application-layer half:
//!
//! - **Local-execution tool bodies** (bash / shell / fs / git / grep / find / ls /
//!   outline / truncate) live HERE, not in the engine: they are environment-specific
//!   agent capabilities — the local ones may become remote sandbox execution later —
//!   so the engine must not depend on them.
//! - **Web tools** (`web_fetch` / `web_search`) are app-layer capabilities too (they
//!   need external credentials/configuration).
//! - **Assembly**: `default_tools` / `subagent_read_only_tools` / the per-subagent
//!   tool-set resolver / `session_tool_set` wire engine tools + local tools together.
//!
//! Subagent tool sets are injected into the engine (`SubagentSpec` carries no tool
//! factory): [`subagent_tool_sets`] builds the resolver `task_tool` and the DAG node
//! launcher are constructed with.

use std::path::PathBuf;
use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::runtime::graph_engineering::engine::DagEngine;
use theway_core::runtime::subagents::registry::SubagentJobRegistry;
use theway_core::tools::node_launcher::ToolSetResolver;
use theway_core::tools::skill::SkillHarnessCell;
use theway_core::tools::{
    dag_tools, install_skill, node_launcher, remove_skill, set_skill_state, skill, skill_builder,
    task,
};

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

/// Read-only tool set used by spawned subagents (issue #11). No `write`/`edit`/`bash` —
/// a subagent should not mutate the workspace; if it needs to, the parent agent should
/// run the write itself. No web tools either (subagents are intentionally scoped to
/// local read-only research).
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
pub fn task_tool(
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    registry: SubagentJobRegistry,
    session_id: Option<String>,
) -> Arc<dyn AgentTool> {
    Arc::new(
        task::TaskTool::new(
            model,
            stream_fn,
            Arc::new(subagent_read_only_tools),
            registry,
        )
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
/// harness (the skill family wires a fresh harness cell per build). Process-level tool
/// groups (`default_tools`, MCP tools) are the caller's to add.
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
    // DAG tools, main agent only — the read-only subagent tool set stays deliberately
    // untouched (shell/exec already ship via `default_tools`).
    tools.extend(dag_tools::DagTools::new(
        dag_engine.clone(),
        Some(session_id.to_string()),
    ));
    // Task delegation tool (issue #11): shares the parent's model + stream backend; jobs
    // are stamped with this session.
    tools.push(task_tool(
        model.clone(),
        stream_fn.cloned(),
        subagent_registry.clone(),
        Some(session_id.to_string()),
    ));
    tools.push(skill_tool(skill_harness_cell.clone()));
    tools.push(install_skill_tool(skill_harness_cell.clone()));
    tools.push(skill_builder_tool(skill_harness_cell.clone()));
    tools.push(set_skill_state_tool(skill_harness_cell.clone()));
    tools.push(remove_skill_tool(skill_harness_cell.clone()));
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

/// Build the `Skill` tool. Separate from `default_tools` because the tool needs to reach the
/// live `AgentHarness::skills()` snapshot, and the harness does not exist yet when this is
/// called (we are still assembling the tool list that will be passed to `AgentHarness::new`).
///
/// The caller (`main.rs`) builds an `Arc<OnceCell<Arc<AgentHarness>>>`, passes it here, and —
/// crucially — sets the cell immediately after the harness is constructed and *before* the
/// REPL accepts any input. If the cell is unset at execute time the tool returns a recoverable
/// `AgentToolError`, never a panic.
pub fn skill_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(skill::SkillTool::new(harness_cell))
}

/// Build the `InstallSkill` tool. Same harness-cell wiring as `skill_tool` because install
/// must hot-reload the catalog via `AgentHarness::reload_skills_from_disk` after writing.
/// See `install_skill::InstallSkillTool` for the two-phase safety model
/// (preview → confirm) and the security note about the in-flight
/// `PermissionCategory::ControlPlaneWrite` plumbing.
pub fn install_skill_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(install_skill::InstallSkillTool::new(harness_cell))
}

/// Build the `SkillBuilder` tool (author a NEW user skill from structured fields). Same
/// harness-cell wiring as `install_skill_tool` — it shares InstallSkill's validation and
/// atomic-write path and hot-reloads the catalog after writing. Where InstallSkill ingests
/// an existing `SKILL.md`, SkillBuilder renders the canonical template itself. See
/// `skill_builder::SkillBuilderTool` for the two-phase preview → confirm model.
pub fn skill_builder_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(skill_builder::SkillBuilderTool::new(harness_cell))
}

/// Build the `SetSkillState` tool (enable/disable a loaded skill at runtime). Same
/// harness-cell wiring as `skill_tool` / `install_skill_tool` — it reads the live catalog,
/// writes the `~/.theway/skills-state.json` overlay, and hot-reloads via
/// `reload_skills_from_disk`. See `set_skill_state::SetSkillStateTool` for the overlay model.
pub fn set_skill_state_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(set_skill_state::SetSkillStateTool::new(harness_cell))
}

/// Build the `RemoveSkill` tool (delete a user-installed skill). Same harness-cell wiring;
/// deletes `~/.theway/skills/<name>/`, clears the overlay entry, and hot-reloads. Builtin/project
/// skills are refused (disable instead). See `remove_skill::RemoveSkillTool`.
pub fn remove_skill_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(remove_skill::RemoveSkillTool::new(harness_cell))
}
