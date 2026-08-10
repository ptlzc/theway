//! Model-facing trigger/cron tool ASSEMBLY (injection-style constructors).
//!
//! Kept separate from `crate::tools` (the main assembly layer) so integration tests
//! (`tests/tools.rs`) can pull this file in by `#[path]` and exercise the constructors
//! against the SAME registry instances their `crate::triggers` path-include clears via
//! the `cfg(test)`-only `clear_for_tests` — `crate::tools` also declares tool-body
//! modules that would not resolve under a `#[path]` include.

use std::sync::Arc;

use theway_core::AgentTool;
use theway_core::tools::skill::SkillHarnessCell;

/// Build the session-scoped cron creation tool. This is the model-facing counterpart to
/// `/cron add`: when the user asks in ordinary conversation for a scheduled / recurring
/// job, the model can register it without falling back to a dynamic trigger.
pub fn new_cron_job_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
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
pub fn remove_cron_job_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
    Arc::new(crate::triggers::RemoveCronJobTool::new(Some(harness_cell)))
}

/// Build the session-scoped cron state tool. This lets the model disable a cron job without
/// deleting the schedule/action text; enable fails closed to `/cron enable` until
/// control-plane confirmation is wired for model-facing writes.
pub fn set_cron_job_state_tool(harness_cell: SkillHarnessCell) -> Arc<dyn AgentTool> {
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
