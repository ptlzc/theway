//! Hook event taxonomy and agent/harness-event → [`EventData`] mapping.
//!
//! Split out of `hooks.rs`; the [`super::HookRunner`] that consumes this data and the
//! rule definitions live in the parent module.

use theway_core::{AgentEvent, HarnessEvent};

use super::utils::{
    assistant_event_name, compaction_trigger, message_kind, message_summary, result_summary,
    truncate,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        match s {
            "agent_start" => Some(Self::AgentStart),
            "agent_end" => Some(Self::AgentEnd),
            "turn_start" => Some(Self::TurnStart),
            "turn_end" => Some(Self::TurnEnd),
            "message_start" => Some(Self::MessageStart),
            "message_update" => Some(Self::MessageUpdate),
            "message_end" => Some(Self::MessageEnd),
            "tool_start" => Some(Self::ToolStart),
            "tool_update" => Some(Self::ToolUpdate),
            "tool_end" => Some(Self::ToolEnd),
            "compaction" => Some(Self::Compaction),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::AgentStart => "agent_start",
            Self::AgentEnd => "agent_end",
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::MessageStart => "message_start",
            Self::MessageUpdate => "message_update",
            Self::MessageEnd => "message_end",
            Self::ToolStart => "tool_start",
            Self::ToolUpdate => "tool_update",
            Self::ToolEnd => "tool_end",
            Self::Compaction => "compaction",
        }
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
    pub(super) fn from_agent_event(event: &AgentEvent) -> Option<Self> {
        match event {
            AgentEvent::AgentStart => Some(Self::basic(HookEvent::AgentStart)),
            AgentEvent::AgentEnd { .. } => Some(Self::basic(HookEvent::AgentEnd)),
            AgentEvent::TurnStart => Some(Self::basic(HookEvent::TurnStart)),
            AgentEvent::TurnEnd { message, .. } => {
                let mut d = Self::basic(HookEvent::TurnEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::MessageStart { message } => {
                let mut d = Self::basic(HookEvent::MessageStart);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => {
                let mut d = Self::basic(HookEvent::MessageUpdate);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                d.assistant_event = Some(assistant_event_name(assistant_message_event).into());
                Some(d)
            }
            AgentEvent::MessageEnd { message } => {
                let mut d = Self::basic(HookEvent::MessageEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::ToolExecutionStart {
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
            AgentEvent::ToolExecutionUpdate {
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
            AgentEvent::ToolExecutionEnd {
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
            AgentEvent::ControlPlanePromptResolved { .. } => None,
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

    pub(super) fn from_harness_event(event: &HarnessEvent) -> Option<Self> {
        match event {
            HarnessEvent::Compaction {
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
            HarnessEvent::SessionStart { .. }
            | HarnessEvent::Branch { .. }
            | HarnessEvent::PersistenceError { .. }
            | HarnessEvent::TurnEnded { .. }
            | HarnessEvent::SkillsReloaded { .. } => None,
        }
    }
}

/// Outcome of one `run_command` race. Lifted out of `tokio::select!` so the match in
/// `HookRunner::run_command` can spell the kill-tree path explicitly per branch rather
/// than mixing it into the select arms.
pub(super) enum HookOutcome {
    Completed(std::io::Result<std::process::Output>),
    TimedOut,
    Cancelled,
}
