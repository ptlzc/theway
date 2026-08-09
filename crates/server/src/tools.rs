//! Tool ASSEMBLY layer (openspec tools-into-core): injection-style constructors that wire
//! runtime objects (model / stream backend / DAG engine / subagent registry / skill harness
//! cell / cron+trigger registries) into the builtin tool bodies. The tool bodies themselves
//! live in the engine crate at [`theway_core::runtime::tools`]; this module is the
//! application-layer composition half of that split.
//!
//! Pure tool-set constructors shared with engine code (`default_tools`,
//! `subagent_read_only_tools`) stayed in [`theway_core::runtime::tools`] and are
//! re-exported here so call sites keep one import path.

use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::runtime::tools::{
    dag_tools, install_skill, remove_skill, set_skill_state, skill, skill_builder, task,
};

pub use theway_core::tools::{default_tools, subagent_read_only_tools};

/// Build the Task tool. Separate from `default_tools` because Task needs the model handle to
/// spawn its inner harness; the caller wires it in at construction time. `session_id`
/// (session-resource-model) stamps the owning session on every spawned job — each harness
/// build gets its own TaskTool stamped with that harness's session.
pub fn task_tool(
    model: theway_llm_provider::Model,
    stream_fn: Option<theway_core::StreamFn>,
    registry: theway_core::runtime::subagents::registry::SubagentJobRegistry,
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

/// Build the per-session tool set (session-resource-model). One source of truth shared by
/// the CLI's initial harness build and the session factory ([`crate::session_ops::SessionFactory`]):
/// everything here is either session-stamped (`dag_*` / `task`) or must be rebuilt per
/// harness (the skill family wires a fresh harness cell per build). Process-level tool
/// groups (`default_tools`, MCP tools) are the caller's to add.
pub fn session_tool_set(
    memory_dir: &std::path::Path,
    dag_engine: &std::sync::Arc<theway_core::runtime::graph_engineering::engine::DagEngine>,
    subagent_registry: &theway_core::runtime::subagents::registry::SubagentJobRegistry,
    model: &theway_llm_provider::Model,
    stream_fn: Option<&theway_core::StreamFn>,
    skill_harness_cell: &skill::SkillHarnessCell,
    session_id: &str,
) -> Vec<Arc<dyn AgentTool>> {
    let mut tools = default_tools(memory_dir.to_path_buf());
    // DAG + outline tools, main agent only — the read-only subagent tool set stays
    // deliberately untouched (shell/exec already ship via `default_tools`).
    tools.push(Arc::new(theway_core::tools::outline::OutlineTool));
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
pub fn skill_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(skill::SkillTool::new(harness_cell))
}

/// Build the `InstallSkill` tool. Same harness-cell wiring as `skill_tool` because install
/// must hot-reload the catalog via `AgentHarness::reload_skills_from_disk` after writing.
/// See `install_skill::InstallSkillTool` for the two-phase safety model
/// (preview → confirm) and the security note about the in-flight
/// `PermissionCategory::ControlPlaneWrite` plumbing.
pub fn install_skill_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(install_skill::InstallSkillTool::new(harness_cell))
}

/// Build the `SkillBuilder` tool (author a NEW user skill from structured fields). Same
/// harness-cell wiring as `install_skill_tool` — it shares InstallSkill's validation and
/// atomic-write path and hot-reloads the catalog after writing. Where InstallSkill ingests
/// an existing `SKILL.md`, SkillBuilder renders the canonical template itself. See
/// `skill_builder::SkillBuilderTool` for the two-phase preview → confirm model.
pub fn skill_builder_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(skill_builder::SkillBuilderTool::new(harness_cell))
}

/// Build the `SetSkillState` tool (enable/disable a loaded skill at runtime). Same
/// harness-cell wiring as `skill_tool` / `install_skill_tool` — it reads the live catalog,
/// writes the `~/.theway/skills-state.json` overlay, and hot-reloads via
/// `reload_skills_from_disk`. See `set_skill_state::SetSkillStateTool` for the overlay model.
pub fn set_skill_state_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(set_skill_state::SetSkillStateTool::new(harness_cell))
}

/// Build the `RemoveSkill` tool (delete a user-installed skill). Same harness-cell wiring;
/// deletes `~/.theway/skills/<name>/`, clears the overlay entry, and hot-reloads. Builtin/project
/// skills are refused (disable instead). See `remove_skill::RemoveSkillTool`.
pub fn remove_skill_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(remove_skill::RemoveSkillTool::new(harness_cell))
}

/// Build the session-scoped cron creation tool. This is the model-facing counterpart to
/// `/cron add`: when the user asks in ordinary conversation for a scheduled / recurring
/// job, the model can register it without falling back to a dynamic trigger.
pub fn new_cron_job_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::NewCronJobTool::new(Some(harness_cell)))
}

/// Build the session-scoped cron listing tool. This is the model-facing counterpart to
/// `/cron list` and returns redacted previews rather than raw action text.
pub fn list_cron_jobs_tool() -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::ListCronJobsTool)
}

/// Build the session-scoped cron removal tool. This is the model-facing counterpart to
/// `/cron remove`: it previews by default and only removes after explicit confirmation,
/// then writes the same control-plane audit as slash commands.
pub fn remove_cron_job_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::RemoveCronJobTool::new(Some(harness_cell)))
}

/// Build the session-scoped cron state tool. This lets the model disable a cron job without
/// deleting the schedule/action text; enable fails closed to `/cron enable` until
/// control-plane confirmation is wired for model-facing writes.
pub fn set_cron_job_state_tool(harness_cell: skill::SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::SetCronJobStateTool::new(Some(
        harness_cell,
    )))
}

/// Build the dynamic trigger creation tool. This is model-facing counterpart to the
/// `/new-trigger` slash command: when the user asks in ordinary conversation to create an
/// automation, the model can register the rule without requiring slash-command syntax.
pub fn new_trigger_tool() -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::NewTriggerTool)
}

/// Build the dynamic trigger listing tool. This is the model-facing counterpart to
/// `/triggers rules`: it lets the assistant inspect current rule ids before answering or
/// removing a rule.
pub fn list_triggers_tool() -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::ListTriggersTool)
}

/// Build the dynamic trigger removal tool. This is the model-facing counterpart to
/// `/triggers remove`: when the user asks in ordinary conversation to delete a trigger,
/// the model can remove the rule by id or clear all rules when explicitly requested.
pub fn remove_trigger_tool() -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::RemoveTriggerTool)
}

/// Build the dynamic trigger state tool. This lets the model pause/resume a trigger without
/// deleting the rule and losing its condition/action text.
pub fn set_trigger_state_tool() -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::SetTriggerStateTool)
}
