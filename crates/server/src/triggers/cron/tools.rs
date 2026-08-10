//! Model-facing tools for session cron jobs: create, list, remove, and enable/disable.

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::errors::{
    cron_job_details_for_model, normalize_schedule, preview_redacted, render_cron_jobs_for_tool,
    write_tool_cron_control_audit,
};
use super::{HarnessCell, MAX_ACTION_PREVIEW_CHARS, global_cron_registry};

pub struct NewCronJobTool {
    harness: Option<HarnessCell>,
}

pub struct ListCronJobsTool;

pub struct RemoveCronJobTool {
    harness: Option<HarnessCell>,
}

pub struct SetCronJobStateTool {
    harness: Option<HarnessCell>,
}

impl NewCronJobTool {
    pub fn new(harness: Option<HarnessCell>) -> Self {
        Self { harness }
    }
}

impl RemoveCronJobTool {
    pub fn new(harness: Option<HarnessCell>) -> Self {
        Self { harness }
    }
}

impl SetCronJobStateTool {
    pub fn new(harness: Option<HarnessCell>) -> Self {
        Self { harness }
    }
}

#[async_trait]
impl AgentTool for NewCronJobTool {
    fn definition(&self) -> &Tool {
        &NEW_CRON_JOB_TOOL
    }

    fn label(&self) -> &str {
        "NewCronJob"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let schedule = params
            .get("schedule")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing required arg: schedule"))?;
        let schedule = normalize_schedule(schedule)?;
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing required arg: action"))?;
        let stateful = params
            .get("stateful")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let job = global_cron_registry()
            .add_job_full(&schedule, action, stateful)
            .map_err(|e| AgentToolError::Message(e.to_string()))?;

        let audit_entry_id =
            write_tool_cron_control_audit(&self.harness, "add", None, Some(&job)).await;

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "created cron job {}\nschedule: {}\naction: {}",
                job.id,
                job.schedule,
                preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
            ))],
            details: json!({
                "id": job.id,
                "schedule": job.schedule,
                "action": job.action,
                "enabled": job.enabled,
                "stateful": job.stateful,
                "scope": "session",
                "audit_entry_id": audit_entry_id,
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for ListCronJobsTool {
    fn definition(&self) -> &Tool {
        &LIST_CRON_JOBS_TOOL
    }

    fn label(&self) -> &str {
        "ListCronJobs"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        _params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let jobs = global_cron_registry().list();
        let storage_path = global_cron_registry()
            .storage_path()
            .map(|path| path.display().to_string());
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(render_cron_jobs_for_tool(&jobs))],
            details: json!({
                "count": jobs.len(),
                "scope": "session",
                "storage_path": storage_path,
                "jobs": jobs.iter().map(cron_job_details_for_model).collect::<Vec<_>>(),
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for RemoveCronJobTool {
    fn definition(&self) -> &Tool {
        &REMOVE_CRON_JOB_TOOL
    }

    fn label(&self) -> &str {
        "RemoveCronJob"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing required arg: id"))?;
        let job = global_cron_registry()
            .list()
            .into_iter()
            .find(|job| job.id == id);
        let Some(job) = job else {
            return Err(AgentToolError::Message(format!(
                "no cron job with id '{id}'"
            )));
        };
        let confirm = params
            .get("confirm")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !confirm {
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(format!(
                    "remove cron job {} requires confirmation\nschedule: {}\naction: {}\ncall RemoveCronJob again with confirm=true only after the user confirms",
                    job.id,
                    job.schedule,
                    preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
                ))],
                details: json!({
                    "id": job.id,
                    "removed_count": 0,
                    "confirmation_required": true,
                    "scope": "session",
                    "action_preview": preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS),
                }),
                terminate: None,
            });
        }

        let removed = global_cron_registry()
            .remove_job(id)
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let Some(job) = removed else {
            return Err(AgentToolError::Message(format!(
                "no cron job with id '{id}'"
            )));
        };

        let audit_entry_id =
            write_tool_cron_control_audit(&self.harness, "remove", Some(&job), None).await;
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "removed cron job {}\nschedule: {}\naction: {}",
                job.id,
                job.schedule,
                preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
            ))],
            details: json!({
                "id": job.id,
                "removed_count": 1,
                "scope": "session",
                "audit_entry_id": audit_entry_id,
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for SetCronJobStateTool {
    fn definition(&self) -> &Tool {
        &SET_CRON_JOB_STATE_TOOL
    }

    fn label(&self) -> &str {
        "SetCronJobState"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Sequential)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing required arg: id"))?;
        let enabled = params
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or_else(|| AgentToolError::from("missing required arg: enabled"))?;
        if enabled {
            return Err(AgentToolError::Message(
                "enabling cron jobs from model-facing tools requires user confirmation; use /cron enable <id>"
                    .into(),
            ));
        }
        let before = global_cron_registry()
            .list()
            .into_iter()
            .find(|job| job.id == id);
        let updated = global_cron_registry()
            .set_job_enabled(id, enabled)
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let Some(job) = updated else {
            return Err(AgentToolError::Message(format!(
                "no cron job with id '{id}'"
            )));
        };

        let op = if enabled { "enable" } else { "disable" };
        let audit_entry_id =
            write_tool_cron_control_audit(&self.harness, op, before.as_ref(), Some(&job)).await;
        let state = if job.enabled { "enabled" } else { "disabled" };
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "updated cron job {}\nstate: {}\nschedule: {}\naction: {}",
                job.id,
                state,
                job.schedule,
                preview_redacted(&job.action, MAX_ACTION_PREVIEW_CHARS)
            ))],
            details: json!({
                "id": job.id,
                "schedule": job.schedule,
                "enabled": job.enabled,
                "stateful": job.stateful,
                "scope": "session",
                "audit_entry_id": audit_entry_id,
            }),
            terminate: None,
        })
    }
}

static NEW_CRON_JOB_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| Tool {
    name: "NewCronJob".into(),
    description: "Create a session-scoped cron scheduled job. Use this when the user asks for a \
         fixed time, recurring, scheduled, hourly, daily, weekly, crontab, 定时任务, 每小时, \
         每天, or similar time-based job. Do not use NewTrigger for these scheduled jobs. \
         Cron jobs are scoped to the current chat session by default."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "schedule": {
                "type": "string",
                "description": "A 5-field cron expression in local time (minute hour day-of-month month day-of-week), or a supported alias such as hourly / every hour / 每小时."
            },
            "action": {
                "type": "string",
                "description": "Natural-language instruction to run when the schedule is due."
            },
            "stateful": {
                "type": "boolean",
                "default": false,
                "description": "Loop mode: run in a fresh sub-agent that keeps persistent notes across runs (injected each time) and routes findings to the triage inbox instead of the chat. Use for recurring watch/triage jobs like \"check for new issues and report only what changed\"."
            }
        },
        "required": ["schedule", "action"],
        "additionalProperties": false,
    }),
});

static LIST_CRON_JOBS_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| Tool {
    name: "ListCronJobs".into(),
    description: "List the session-scoped cron scheduled jobs. Use this when the user asks to \
         view, list, inspect, or find scheduled jobs, cron jobs, crontab entries, 定时任务, \
         or recurring jobs."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    }),
});

static REMOVE_CRON_JOB_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| Tool {
    name: "RemoveCronJob".into(),
    description: "Preview or confirm removal of a session-scoped cron scheduled job by exact id. \
         Use confirm=false first when the user asks to delete, remove, or clear a scheduled job, \
         cron job, crontab entry, or 定时任务. Call confirm=true only after the user explicitly \
         confirms removal."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Exact cron job id, for example cron-abc123."
            },
            "confirm": {
                "type": "boolean",
                "description": "false to preview the removal; true only after explicit user confirmation."
            }
        },
        "required": ["id"],
        "additionalProperties": false,
    }),
});

static SET_CRON_JOB_STATE_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| Tool {
    name: "SetCronJobState".into(),
    description: "Disable a session-scoped cron scheduled job by exact id. Model-facing \
             enable/resume is refused until control-plane confirmation is wired; use \
             /cron enable <id> for enabling."
        .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Exact cron job id, for example cron-abc123."
            },
            "enabled": {
                "type": "boolean",
                "description": "true to enable/resume the cron job; false to disable/pause it."
            }
        },
        "required": ["id", "enabled"],
        "additionalProperties": false,
    }),
});
