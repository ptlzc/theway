use serde::Deserialize;
use theway_contract::extension::{
    ExtensionActionKind, ExtensionErrorEnvelope, ExtensionGateDecision, ExtensionHookClass,
    ExtensionLifecycleEvent,
};
use tokio_util::sync::CancellationToken;

use crate::agent::runtime_extensions::ValidatedRuntimeExtensionResult;
use crate::types::{
    AfterToolCallContext, AfterToolCallResult, AgentToolResult, BeforeToolCallContext,
    BeforeToolCallResult,
};

use super::HarnessRuntimeExtensions;

impl HarnessRuntimeExtensions {
    pub(crate) async fn before_tool_call(
        &self,
        context: &BeforeToolCallContext,
        cancel: &CancellationToken,
    ) -> BeforeToolCallResult {
        let tool_call_id = context.tool_call.id.clone();
        let source_message_id = { self.active.lock().source_message_id.clone() };
        let invocation = self.invocation_scoped(
            ExtensionLifecycleEvent::ToolCall,
            ExtensionHookClass::Gate,
            serde_json::json!({
                "assistantMessage": context.assistant_message,
                "toolCall": context.tool_call,
                "args": context.args,
            }),
            cancel.is_cancelled(),
            source_message_id,
            Some(tool_call_id),
        );
        let result = match invocation {
            Ok(invocation) => self.guarded(self.port.dispatch_tool(invocation)).await,
            Err(error) => Err(error),
        };
        let result = match result {
            Ok(ValidatedRuntimeExtensionResult::Gate(result)) => result,
            Ok(_) => {
                return blocked(
                    "contract_violation",
                    "tool gate returned the wrong hook class",
                );
            }
            Err(error) => return blocked_error(error),
        };
        if !result.actions().is_empty() {
            return blocked(
                "contract_violation",
                "tool gate actions require the durable action coordinator",
            );
        }
        match result.decision() {
            ExtensionGateDecision::Abstain | ExtensionGateDecision::Allow => {
                BeforeToolCallResult::default()
            }
            ExtensionGateDecision::Deny { code, message }
            | ExtensionGateDecision::Cancel { code, message } => blocked(code, message),
        }
    }

    pub(crate) async fn transform_tool_result(
        &self,
        context: &AfterToolCallContext,
        cancel: &CancellationToken,
    ) -> AfterToolCallResult {
        let source_message_id = { self.active.lock().source_message_id.clone() };
        let invocation = self.invocation_scoped(
            ExtensionLifecycleEvent::ToolResult,
            ExtensionHookClass::Transform,
            serde_json::json!({
                "toolCall": context.tool_call,
                "args": context.args,
                "result": context.result,
                "isError": context.is_error,
            }),
            cancel.is_cancelled(),
            source_message_id,
            Some(context.tool_call.id.clone()),
        );
        let Ok(invocation) = invocation else {
            return AfterToolCallResult::default();
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_tool(invocation)).await
        else {
            return AfterToolCallResult::default();
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::ReplaceToolResult | ExtensionActionKind::EnqueueFollowUp
            )
        }) {
            return AfterToolCallResult::default();
        }
        let replacement = match result
            .actions()
            .iter()
            .find(|action| action.kind == ExtensionActionKind::ReplaceToolResult)
        {
            Some(action) => {
                match serde_json::from_value::<ToolResultReplacement>(action.payload.clone()) {
                    Ok(replacement) => Some(replacement),
                    Err(_) => return AfterToolCallResult::default(),
                }
            }
            None => None,
        };
        let follow_ups = result
            .actions()
            .iter()
            .filter(|action| action.kind == ExtensionActionKind::EnqueueFollowUp)
            .map(|action| super::parse_follow_up(&action.payload))
            .collect::<Option<Vec<_>>>();
        let Some(follow_ups) = follow_ups else {
            return AfterToolCallResult::default();
        };
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return AfterToolCallResult::default();
        }
        replacement.map_or_else(AfterToolCallResult::default, |replacement| {
            AfterToolCallResult {
                content: Some(replacement.result.content),
                details: Some(replacement.result.details),
                is_error: replacement.is_error,
                terminate: Some(replacement.result.terminate.unwrap_or(false)),
            }
        })
    }

    pub(super) async fn observe_tool_execution(
        &self,
        event: ExtensionLifecycleEvent,
        tool_call_id: String,
        payload: serde_json::Value,
        cancel: &CancellationToken,
    ) {
        let source_message_id = { self.active.lock().source_message_id.clone() };
        let Ok(invocation) = self.invocation_scoped(
            event,
            ExtensionHookClass::Observe,
            payload,
            cancel.is_cancelled(),
            source_message_id,
            Some(tool_call_id),
        ) else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_tool(invocation)).await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolResultReplacement {
    result: AgentToolResult,
    #[serde(default)]
    is_error: Option<bool>,
}

fn blocked(code: &str, message: &str) -> BeforeToolCallResult {
    BeforeToolCallResult {
        block: true,
        reason: Some(format!("extension tool gate denied [{code}]: {message}")),
        prompt: None,
    }
}

fn blocked_error(error: ExtensionErrorEnvelope) -> BeforeToolCallResult {
    let code = serde_json::to_value(error.code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "internal".into());
    blocked(&code, &error.message)
}
