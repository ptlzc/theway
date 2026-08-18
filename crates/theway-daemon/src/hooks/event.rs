//! Hook event taxonomy and agent/harness-event → [`EventData`] mapping.
//!
//! Split out of `hooks.rs`; the [`super::HookRunner`] that consumes this data and the
//! rule definitions live in the parent module.

use theway_core::{LoopEvent, SessionEvent};

use strum::{EnumString, IntoStaticStr};

use super::utils::{
    assistant_event_name, compaction_trigger, message_kind, message_summary, result_summary,
    truncate,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(super) enum HookEvent {
    AgentStart,
    AgentEnd,
    TurnStart,
    TurnEnd,
    MessageStart,
    MessageUpdate,
    MessageEnd,
    ToolStart,
    ToolUpdate,
    ToolEnd,
    Compaction,
}

impl HookEvent {
    pub(super) fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    pub(super) fn as_str(self) -> &'static str {
        self.into()
    }
}

pub(super) struct EventData {
    pub(super) event: HookEvent,
    pub(super) message_kind: Option<String>,
    pub(super) message_summary: Option<String>,
    pub(super) assistant_event: Option<String>,
    pub(super) tool_call_id: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) tool_is_error: Option<bool>,
    pub(super) tool_args: Option<serde_json::Value>,
    pub(super) tool_result_summary: Option<String>,
    pub(super) compaction_trigger: Option<String>,
    pub(super) compaction_tokens_before: Option<u64>,
    pub(super) compaction_summary: Option<String>,
}

impl EventData {
    pub(super) fn from_agent_event(event: &LoopEvent) -> Option<Self> {
        match event {
            LoopEvent::RunStarted => Some(Self::basic(HookEvent::AgentStart)),
            LoopEvent::RunEnded { .. } => Some(Self::basic(HookEvent::AgentEnd)),
            LoopEvent::TurnStart => Some(Self::basic(HookEvent::TurnStart)),
            LoopEvent::TurnCompleted { message, .. } => {
                let mut d = Self::basic(HookEvent::TurnEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            LoopEvent::MessageStart { message } => {
                let mut d = Self::basic(HookEvent::MessageStart);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            LoopEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => {
                let mut d = Self::basic(HookEvent::MessageUpdate);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                d.assistant_event = Some(assistant_event_name(assistant_message_event).into());
                Some(d)
            }
            LoopEvent::MessageEnd { message } => {
                let mut d = Self::basic(HookEvent::MessageEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            LoopEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let mut d = Self::basic(HookEvent::ToolStart);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_args = Some(args.clone());
                Some(d)
            }
            LoopEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
            } => {
                let mut d = Self::basic(HookEvent::ToolUpdate);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_args = Some(args.clone());
                d.tool_result_summary = Some(result_summary(partial_result));
                Some(d)
            }
            LoopEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let mut d = Self::basic(HookEvent::ToolEnd);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_is_error = Some(*is_error);
                d.tool_result_summary = Some(result_summary(result));
                Some(d)
            }
            // Issue #110: control-plane prompt observability event. Embedder-side
            // hook-script bridge does not currently surface this; defer to a follow-up
            // PR once we have a concrete bridge consumer that wants it.
            LoopEvent::ControlPlanePromptResolved { .. } => None,
        }
    }

    fn basic(event: HookEvent) -> Self {
        Self {
            event,
            message_kind: None,
            message_summary: None,
            assistant_event: None,
            tool_call_id: None,
            tool_name: None,
            tool_is_error: None,
            tool_args: None,
            tool_result_summary: None,
            compaction_trigger: None,
            compaction_tokens_before: None,
            compaction_summary: None,
        }
    }

    pub(super) fn from_harness_event(event: &SessionEvent) -> Option<Self> {
        match event {
            SessionEvent::Compaction {
                from_hook,
                summary,
                tokens_before,
            } => {
                let mut d = Self::basic(HookEvent::Compaction);
                d.compaction_trigger = Some(compaction_trigger(*from_hook).into());
                d.compaction_tokens_before = Some(*tokens_before);
                d.compaction_summary = Some(truncate(summary));
                Some(d)
            }
            SessionEvent::Started { .. }
            | SessionEvent::Branch { .. }
            | SessionEvent::PersistenceError { .. }
            | SessionEvent::TurnDecision { .. }
            | SessionEvent::SkillsReloaded { .. } => None,
        }
    }
}

#[cfg(test)]
// Test files live in `tests/hooks/event/` (mirror of src), pulled in by path so
// they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("hooks/event");
