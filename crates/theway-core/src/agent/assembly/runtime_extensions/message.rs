use theway_contract::extension::{
    ExtensionActionKind, ExtensionHookClass, ExtensionLifecycleEvent,
};
use tokio_util::sync::CancellationToken;

use crate::agent::runtime_extensions::{
    RuntimeExtensionScopeKind, ValidatedRuntimeExtensionResult,
};
use crate::types::AgentMessage;
use theway_llm_provider::AssistantMessageEvent;

use super::{HarnessRuntimeExtensions, is_host_consumed_action};

impl HarnessRuntimeExtensions {
    pub(crate) async fn finalize_before_run_message(&self, message: AgentMessage) -> AgentMessage {
        let cancel = CancellationToken::new();
        self.observe_message_start(&message, &cancel).await;
        let message = self.transform_message(message, cancel).await;
        self.complete_message(&message);
        message
    }

    pub(super) async fn observe_message_start(
        &self,
        message: &AgentMessage,
        cancel: &CancellationToken,
    ) {
        let Ok(message_id) = self.allocate(RuntimeExtensionScopeKind::Message) else {
            return;
        };
        self.active.lock().message_id = Some(message_id.clone());
        self.observe_message(
            ExtensionLifecycleEvent::MessageStart,
            serde_json::json!({"message": message}),
            cancel,
            message_id,
        )
        .await;
    }

    pub(super) async fn observe_message_update(
        &self,
        message: &AgentMessage,
        update: &AssistantMessageEvent,
        cancel: &CancellationToken,
    ) {
        let Some(message_id) = self.active.lock().message_id.clone() else {
            return;
        };
        self.observe_message(
            ExtensionLifecycleEvent::MessageUpdate,
            serde_json::json!({
                "message": message,
                "updateKind": assistant_update_kind(update),
            }),
            cancel,
            message_id,
        )
        .await;
    }

    pub(crate) async fn transform_message(
        &self,
        message: AgentMessage,
        cancel: CancellationToken,
    ) -> AgentMessage {
        let active_message_id = { self.active.lock().message_id.clone() };
        let message_id = match active_message_id {
            Some(id) => id,
            None => {
                self.observe_message_start(&message, &cancel).await;
                let Some(id) = self.active.lock().message_id.clone() else {
                    return message;
                };
                id
            }
        };
        let Ok(invocation) = self.invocation_scoped(
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionHookClass::Transform,
            serde_json::json!({"message": message}),
            cancel.is_cancelled(),
            Some(message_id),
            None,
        ) else {
            return message;
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_message(invocation)).await
        else {
            return message;
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::ReplaceMessage | ExtensionActionKind::EnqueueFollowUp
            ) && !is_host_consumed_action(action.kind)
        }) {
            return message;
        }

        let original = message.clone();
        let replacement = match result
            .actions()
            .iter()
            .find(|action| action.kind == ExtensionActionKind::ReplaceMessage)
        {
            Some(action) => {
                let Some(value) = action.payload.get("message") else {
                    return original;
                };
                let Ok(message) = serde_json::from_value::<AgentMessage>(value.clone()) else {
                    return original;
                };
                Some(message)
            }
            None => None,
        };
        let replacement = replacement.unwrap_or_else(|| original.clone());
        if !super::same_message_role(&original, &replacement) {
            return original;
        }
        let follow_ups = result
            .actions()
            .iter()
            .filter(|action| action.kind == ExtensionActionKind::EnqueueFollowUp)
            .map(|action| super::parse_follow_up(&action.payload))
            .collect::<Option<Vec<_>>>();
        let Some(follow_ups) = follow_ups else {
            return original;
        };
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return original;
        }
        replacement
    }

    pub(super) fn complete_message(&self, message: &AgentMessage) {
        let mut active = self.active.lock();
        let message_id = active.message_id.take();
        if matches!(
            message,
            AgentMessage::Llm(theway_llm_provider::Message::Assistant(_))
        ) {
            active.source_message_id = message_id;
        }
    }

    async fn observe_message(
        &self,
        event: ExtensionLifecycleEvent,
        payload: serde_json::Value,
        cancel: &CancellationToken,
        message_id: String,
    ) {
        let Ok(invocation) = self.invocation_scoped(
            event,
            ExtensionHookClass::Observe,
            payload,
            cancel.is_cancelled(),
            Some(message_id),
            None,
        ) else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_message(invocation)).await;
    }
}

fn assistant_update_kind(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start { .. } => "start",
        AssistantMessageEvent::TextStart { .. } => "text_start",
        AssistantMessageEvent::TextDelta { .. } => "text_delta",
        AssistantMessageEvent::TextEnd { .. } => "text_end",
        AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        AssistantMessageEvent::ToolCallStart { .. } => "tool_call_start",
        AssistantMessageEvent::ToolCallDelta { .. } => "tool_call_delta",
        AssistantMessageEvent::ToolCallEnd { .. } => "tool_call_end",
        AssistantMessageEvent::Done { .. } => "done",
        AssistantMessageEvent::Error { .. } => "error",
    }
}
