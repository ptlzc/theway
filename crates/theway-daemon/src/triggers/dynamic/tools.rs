//! Model-facing tools for managing dynamic trigger rules: create (`new_trigger`), list
//! (`list_triggers`), remove (`remove_trigger`), and enable/disable (`set_trigger_state`).

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, PermissionClassification,
    ToolExecutionMode,
};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::{DynamicTriggerRule, global_registry};

pub struct NewTriggerTool;

pub struct ListTriggersTool;

pub struct RemoveTriggerTool;

pub struct SetTriggerStateTool;

#[async_trait]
impl AgentTool for NewTriggerTool {
    fn definition(&self) -> &Tool {
        &NEW_TRIGGER_TOOL
    }

    fn label(&self) -> &str {
        "new_trigger"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    /// Issue #110 sub-PR 3 classifier — every new dynamic trigger is a persistent
    /// agent-self-modification: the model attaches a recurring action to a future external
    /// event. Always Prompt. The reason is **value-free by construction** (names the input
    /// fields the model supplied, NOT their content) so a tokenized URL or other
    /// secret-bearing payload smuggled into `condition` / `action` / `spec` cannot leak
    /// through `Prompt.reason` into the audit / UI surface. The full bounded args still flow
    /// through the runtime default `prompt_payload` (`{tool_name, args_keys, args_hash}`)
    /// for the embedder's prompt card.
    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        let has_condition = prepared_args
            .get("condition")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        let has_action = prepared_args
            .get("action")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        let has_spec = prepared_args
            .get("spec")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        let reason = match (has_condition && has_action, has_spec) {
            (true, _) => "create dynamic trigger from `condition` + `action` fields".to_string(),
            (false, true) => "create dynamic trigger from `spec` field".to_string(),
            (false, false) => "create dynamic trigger".to_string(),
        };
        PermissionClassification::Prompt { reason }
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let condition = params.get("condition").and_then(|v| v.as_str());
        let action = params.get("action").and_then(|v| v.as_str());
        let fire_once = params
            .get("fire_once")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let promote_to_chat = params
            .get("promote_to_chat")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let fixed_schedule_text = [
            condition,
            action,
            params.get("spec").and_then(|v| v.as_str()),
        ]
        .into_iter()
        .flatten()
        .any(looks_like_fixed_schedule_request);
        if fixed_schedule_text {
            return Err(AgentToolError::Message(
                "fixed scheduled jobs must use new_cron_job, not new_trigger".into(),
            ));
        }
        let rule = match (condition, action) {
            (Some(condition), Some(action)) => {
                global_registry().add_rule_with_flags(condition, action, fire_once, promote_to_chat)
            }
            _ => {
                let spec = params.get("spec").and_then(|v| v.as_str()).ok_or_else(|| {
                    AgentToolError::from("missing required args: provide condition and action")
                })?;
                global_registry().add_from_spec(spec)
            }
        }
        .map_err(|e| AgentToolError::Message(e.to_string()))?;
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "created dynamic trigger {}\ncondition: {}\naction: {}\nfire_once: {}\npromote_to_chat: {}",
                rule.id, rule.condition, rule.action, rule.fire_once, rule.promote_to_chat
            ))],
            details: json!({
                "id": rule.id,
                "condition": rule.condition,
                "action": rule.action,
                "enabled": rule.enabled,
                "fire_once": rule.fire_once,
                "fired_at": rule.fired_at,
                "promote_to_chat": rule.promote_to_chat,
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for ListTriggersTool {
    fn definition(&self) -> &Tool {
        &LIST_TRIGGERS_TOOL
    }

    fn label(&self) -> &str {
        "list_triggers"
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
        let rules = global_registry().list();
        let storage_path = global_registry()
            .storage_path()
            .map(|path| path.display().to_string());
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(render_trigger_rules_for_tool(
                &rules,
            ))],
            details: json!({
                "count": rules.len(),
                "rules": rules,
                "storage_path": storage_path,
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for RemoveTriggerTool {
    fn definition(&self) -> &Tool {
        &REMOVE_TRIGGER_TOOL
    }

    fn label(&self) -> &str {
        "remove_trigger"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    /// Issue #110 sub-PR 3 classifier — every trigger removal is a destructive
    /// control-plane write. Prompt with a reason that distinguishes single-id removal from
    /// the `all = true` bulk path (which is meaningfully more destructive and gets its own
    /// emphasized reason on the prompt card).
    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        let reason = if prepared_args
            .get("all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "remove ALL dynamic triggers".to_string()
        } else if let Some(id) = prepared_args.get("id").and_then(|v| v.as_str()) {
            format!("remove dynamic trigger `{id}`")
        } else {
            "remove dynamic trigger".to_string()
        };
        PermissionClassification::Prompt { reason }
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        if params.get("all").and_then(|v| v.as_bool()) == Some(true) {
            let count = global_registry()
                .clear_rules()
                .map_err(|e| AgentToolError::Message(e.to_string()))?;
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(format!(
                    "removed {count} dynamic trigger rule(s)"
                ))],
                details: json!({ "removed_count": count, "all": true }),
                terminate: None,
            });
        }

        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing required arg: id"))?;
        let removed = global_registry()
            .remove_rule(id)
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let Some(rule) = removed else {
            return Err(AgentToolError::Message(format!(
                "no dynamic trigger rule with id '{id}'"
            )));
        };
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "removed dynamic trigger {}\ncondition: {}\naction: {}",
                rule.id, rule.condition, rule.action
            ))],
            details: json!({
                "id": rule.id,
                "condition": rule.condition,
                "action": rule.action,
                "removed_count": 1,
            }),
            terminate: None,
        })
    }
}

#[async_trait]
impl AgentTool for SetTriggerStateTool {
    fn definition(&self) -> &Tool {
        &SET_TRIGGER_STATE_TOOL
    }

    fn label(&self) -> &str {
        "set_trigger_state"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    /// Issue #110 sub-PR 3 classifier — same narrowing/escalating split as
    /// `SetSkillStateTool`: disabling an existing trigger is narrowing (the trigger stops
    /// firing) and falls through `Allow`; re-enabling an existing trigger is escalating
    /// (the model re-opens a recurring side-effect path) and routes through the prompt.
    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        let enabled = prepared_args
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !enabled {
            return PermissionClassification::Allow;
        }
        let id = prepared_args
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        PermissionClassification::Prompt {
            reason: format!("re-enable dynamic trigger `{id}`"),
        }
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
        let updated = global_registry()
            .set_rule_enabled(id, enabled)
            .map_err(|e| AgentToolError::Message(e.to_string()))?;
        let Some(rule) = updated else {
            return Err(AgentToolError::Message(format!(
                "no dynamic trigger rule with id '{id}'"
            )));
        };
        let state = if rule.enabled { "enabled" } else { "disabled" };
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "updated dynamic trigger {}\nstate: {}\ncondition: {}\naction: {}",
                rule.id, state, rule.condition, rule.action
            ))],
            details: json!({
                "id": rule.id,
                "condition": rule.condition,
                "action": rule.action,
                "enabled": rule.enabled,
                "fire_once": rule.fire_once,
                "fired_at": rule.fired_at,
                "promote_to_chat": rule.promote_to_chat,
            }),
            terminate: None,
        })
    }
}

fn render_trigger_rules_for_tool(rules: &[DynamicTriggerRule]) -> String {
    if rules.is_empty() {
        return "dynamic trigger rules: none".into();
    }
    let mut lines = vec![format!("dynamic trigger rules: {}", rules.len())];
    for rule in rules {
        let state = if rule.enabled { "enabled" } else { "disabled" };
        let fire_mode = if rule.fire_once {
            "fire_once"
        } else {
            "repeat"
        };
        let output_mode = if rule.promote_to_chat {
            "promote_to_chat"
        } else {
            "audit_only"
        };
        lines.push(format!(
            "- {} [{state}, {fire_mode}, {output_mode}] created_at={} condition: {} action: {}",
            rule.id,
            rule.created_at.to_rfc3339(),
            rule.condition,
            rule.action
        ));
    }
    lines.join("\n")
}

fn looks_like_fixed_schedule_request(text: &str) -> bool {
    let lower = text.to_lowercase();
    let english = [
        "every hour",
        "hourly",
        "every day",
        "daily",
        "every week",
        "weekly",
        "scheduled job",
        "cron",
        "crontab",
    ];
    english.iter().any(|needle| lower.contains(needle))
        || text.contains("定时任务")
        || text.contains("定時任務")
        || text.contains("每小时")
        || text.contains("每小時")
        || text.contains("每天")
        || text.contains("每日")
        || text.contains("每周")
        || text.contains("每週")
}

static NEW_TRIGGER_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| {
    Tool {
        name: "new_trigger".into(),
        description: "Create an event/condition-based dynamic trigger rule. Use this for future events such as a browser tab, file, MCP notification, webhook, or other condition becoming true. Do not use this for fixed time, recurring, scheduled, hourly, daily, weekly, cron, crontab, 定时任务, 每小时, or similar time-based jobs; use new_cron_job instead.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "condition": {
                    "type": "string",
                    "description": "The natural-language condition that should be evaluated against future trigger events.",
                },
                "action": {
                    "type": "string",
                    "description": "The action to perform when the condition matches. This may be a shell command or a natural-language instruction.",
                },
                "spec": {
                    "type": "string",
                    "description": "Fallback complete trigger rule text when condition and action cannot be supplied separately.",
                },
                "fire_once": {
                    "type": "boolean",
                    "description": "Whether to disable the rule after the first successful match. Defaults to true unless the user explicitly asks for a repeating trigger.",
                },
                "promote_to_chat": {
                    "type": "boolean",
                    "description": "Whether successful trigger output should be inserted into the parent chat context so future turns can see it. Defaults to false unless the user explicitly asks for that behavior.",
                }
            },
            "required": ["condition", "action"],
            "additionalProperties": false,
        }),
    }
});

#[cfg(test)]
// Test files live in `tests/triggers/dynamic/tools/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("triggers/dynamic/tools");

static LIST_TRIGGERS_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| {
    Tool {
    name: "list_triggers".into(),
    description: "List dynamic trigger rules currently registered in theway. Use this when the user asks to view, list, show, inspect, or find trigger ids.".into(),
    parameters: json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    }),
}
});

static REMOVE_TRIGGER_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| {
    Tool {
        name: "remove_trigger".into(),
        description: "Delete dynamic trigger rules. Use this when the user asks theway to delete, remove, or clear an existing dynamic trigger.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The exact dynamic trigger rule id to remove.",
                },
                "all": {
                    "type": "boolean",
                    "description": "Set true only when the user explicitly asks to remove all dynamic trigger rules.",
                }
            },
            "additionalProperties": false,
        }),
    }
});

static SET_TRIGGER_STATE_TOOL: once_cell::sync::Lazy<Tool> = once_cell::sync::Lazy::new(|| {
    Tool {
        name: "set_trigger_state".into(),
        description: "Enable or disable an existing dynamic trigger rule without deleting it. Use this when the user asks to pause, disable, enable, or resume a trigger.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The exact dynamic trigger rule id to update.",
                },
                "enabled": {
                    "type": "boolean",
                    "description": "Set false to pause or disable the trigger; set true to enable or resume it.",
                }
            },
            "required": ["id", "enabled"],
            "additionalProperties": false,
        }),
    }
});
