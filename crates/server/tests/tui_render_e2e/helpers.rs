//! Shared fixtures for the TUI render e2e suite: faux assistant-message builders, an
//! `LoopEvent::MessageUpdate` wrapper, and an ANSI-stripping helper so assertions can
//! read the textual content directly.

use std::sync::Arc;

use theway_core::{AgentMessage, AgentTool, LoopEvent};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantRole, ContentBlock, ImageContent, Message,
    StopReason, ToolResultMessage, ToolResultRole, Usage,
};

pub fn assistant(content: Vec<ContentBlock>) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

pub fn message_update(ev: AssistantMessageEvent, partial: AssistantMessage) -> LoopEvent {
    LoopEvent::MessageUpdate {
        message: AgentMessage::Llm(Message::Assistant(partial)),
        assistant_message_event: ev,
    }
}

/// Strip ANSI SGR escapes so assertions can read the textual content directly. Operates on
/// chars to preserve multi-byte UTF-8 glyphs (like the `⚙` gear).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut iter = s.chars().peekable();
    while let Some(c) = iter.next() {
        if c == '\x1b' && iter.peek() == Some(&'[') {
            iter.next(); // consume '['
            // Skip CSI bytes until a final byte in 0x40..=0x7e (i.e. an ASCII letter or '@'..'~').
            while let Some(&peek) = iter.peek() {
                iter.next();
                if ('\x40'..='\x7e').contains(&peek) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

#[allow(dead_code)]
pub fn ensure_imports_used(
    _t: ToolResultMessage,
    _i: ImageContent,
    _r: ToolResultRole,
    _a: Arc<dyn AgentTool>,
) {
}
