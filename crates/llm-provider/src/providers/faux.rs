//! Deterministic test-double provider. 1:1 port of `packages/ai/src/providers/faux.ts`.
//!
//! Tests queue pre-built `AssistantMessage` responses; each `stream()` call pops the next one
//! and replays it as a normal event sequence (Start → per-block start/delta/end → Done). When
//! the queue is empty the provider falls back to a single canned "[faux] hello" message.
//!
//! Builders (`faux_text`, `faux_thinking`, `faux_tool_call`, `faux_assistant_message`) mirror the
//! TS helpers so tests read the same.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use serde_json::Map;

use crate::api_registry::ApiProvider;
use crate::types::*;
use crate::utils::event_stream::{AssistantMessageEventSender, AssistantMessageEventStream};

fn response_queue() -> &'static Mutex<VecDeque<AssistantMessage>> {
    static CELL: OnceLock<Mutex<VecDeque<AssistantMessage>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Replace the queued faux responses.
pub fn set_faux_responses(responses: Vec<AssistantMessage>) {
    let mut q = response_queue().lock().expect("faux queue poisoned");
    *q = responses.into_iter().collect();
}

/// Append responses to the queue.
pub fn append_faux_responses(responses: Vec<AssistantMessage>) {
    let mut q = response_queue().lock().expect("faux queue poisoned");
    q.extend(responses);
}

/// Clear the queue.
pub fn clear_faux_responses() {
    response_queue()
        .lock()
        .expect("faux queue poisoned")
        .clear();
}

// ── builders ──────────────────────────────────────────────────────────────────────────────

pub fn faux_text(text: impl Into<String>) -> ContentBlock {
    ContentBlock::text(text)
}

pub fn faux_thinking(thinking: impl Into<String>) -> ContentBlock {
    ContentBlock::Thinking(ThinkingContent {
        thinking: thinking.into(),
        thinking_signature: None,
        redacted: false,
    })
}

pub fn faux_tool_call(
    name: impl Into<String>,
    arguments: Map<String, serde_json::Value>,
) -> ContentBlock {
    ContentBlock::ToolCall(ToolCall {
        id: format!("faux_{}", uuid::Uuid::new_v4().simple()),
        name: name.into(),
        arguments,
        thought_signature: None,
    })
}

pub fn faux_assistant_message(content: Vec<ContentBlock>) -> AssistantMessage {
    let has_tool = content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolCall(_)));
    AssistantMessage {
        role: AssistantRole::Assistant,
        content,
        api: Api::from("faux"),
        provider: Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: if has_tool {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        },
        error_message: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

#[derive(Default)]
pub struct FauxProvider {}

#[async_trait]
impl ApiProvider for FauxProvider {
    fn api(&self) -> &str {
        "faux"
    }

    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        _options: Option<&StreamOptions>,
    ) -> AssistantMessageEventStream {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let queued = response_queue()
            .lock()
            .expect("faux queue poisoned")
            .pop_front();
        let msg = queued.unwrap_or_else(|| AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text("[faux] hello")],
            api: model.api.clone(),
            provider: model.provider.clone(),
            model: model.id.clone(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: chrono::Utc::now().timestamp_millis(),
        });
        replay(msg, &mut sender);
        stream
    }

    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: Option<&SimpleStreamOptions>,
    ) -> AssistantMessageEventStream {
        let base = options.map(|o| o.base.clone());
        self.stream(model, context, base.as_ref())
    }
}

/// Replay a finished `AssistantMessage` as a normal streaming event sequence.
fn replay(msg: AssistantMessage, sender: &mut AssistantMessageEventSender) {
    // Build the partial incrementally so each event carries a faithful snapshot.
    let mut partial = AssistantMessage {
        content: vec![],
        ..msg.clone()
    };
    partial.stop_reason = StopReason::Stop;
    sender.push(AssistantMessageEvent::Start {
        partial: partial.clone(),
    });

    for (idx, block) in msg.content.iter().enumerate() {
        match block {
            ContentBlock::Text(t) => {
                partial.content.push(ContentBlock::text(""));
                sender.push(AssistantMessageEvent::TextStart {
                    content_index: idx,
                    partial: partial.clone(),
                });
                if let Some(ContentBlock::Text(p)) = partial.content.get_mut(idx) {
                    p.text = t.text.clone();
                }
                sender.push(AssistantMessageEvent::TextDelta {
                    content_index: idx,
                    delta: t.text.clone(),
                    partial: partial.clone(),
                });
                sender.push(AssistantMessageEvent::TextEnd {
                    content_index: idx,
                    content: t.text.clone(),
                    partial: partial.clone(),
                });
            }
            ContentBlock::Thinking(t) => {
                partial
                    .content
                    .push(ContentBlock::Thinking(ThinkingContent::default()));
                sender.push(AssistantMessageEvent::ThinkingStart {
                    content_index: idx,
                    partial: partial.clone(),
                });
                if let Some(ContentBlock::Thinking(p)) = partial.content.get_mut(idx) {
                    p.thinking = t.thinking.clone();
                }
                sender.push(AssistantMessageEvent::ThinkingDelta {
                    content_index: idx,
                    delta: t.thinking.clone(),
                    partial: partial.clone(),
                });
                sender.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: idx,
                    content: t.thinking.clone(),
                    partial: partial.clone(),
                });
            }
            ContentBlock::ToolCall(tc) => {
                partial.content.push(ContentBlock::ToolCall(tc.clone()));
                sender.push(AssistantMessageEvent::ToolCallStart {
                    content_index: idx,
                    partial: partial.clone(),
                });
                sender.push(AssistantMessageEvent::ToolCallDelta {
                    content_index: idx,
                    delta: serde_json::to_string(&tc.arguments).unwrap_or_default(),
                    partial: partial.clone(),
                });
                sender.push(AssistantMessageEvent::ToolCallEnd {
                    content_index: idx,
                    tool_call: tc.clone(),
                    partial: partial.clone(),
                });
            }
            ContentBlock::Image(_) => {
                partial.content.push(block.clone());
            }
        }
    }

    partial.stop_reason = msg.stop_reason;
    partial.usage = msg.usage.clone();
    let reason = match msg.stop_reason {
        StopReason::ToolUse => DoneReason::ToolUse,
        StopReason::Length => DoneReason::Length,
        StopReason::Error => {
            sender.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Error,
                error: msg,
            });
            return;
        }
        StopReason::Aborted => {
            sender.push(AssistantMessageEvent::Error {
                reason: ErrorReason::Aborted,
                error: msg,
            });
            return;
        }
        StopReason::Stop => DoneReason::Stop,
    };
    sender.push(AssistantMessageEvent::Done {
        reason,
        message: msg,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    /// The faux queue is process-global; serialize the tests that touch it.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn faux_model() -> Model {
        Model {
            id: "faux".into(),
            name: "Faux".into(),
            api: Api::from("faux"),
            provider: Provider::from("faux"),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    #[tokio::test]
    async fn replays_queued_text_and_tool_call() {
        let _guard = test_lock();
        clear_faux_responses();
        let mut args = Map::new();
        args.insert("city".into(), json!("sf"));
        set_faux_responses(vec![faux_assistant_message(vec![
            faux_text("hi there"),
            faux_tool_call("weather", args),
        ])]);

        let provider = FauxProvider::default();
        let mut s = provider.stream(&faux_model(), &Context::default(), None);

        let mut text = String::new();
        let mut tool_name = None;
        let mut done_reason = None;
        while let Some(ev) = s.next().await {
            match ev {
                AssistantMessageEvent::TextDelta { delta, .. } => text.push_str(&delta),
                AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                    tool_name = Some(tool_call.name)
                }
                AssistantMessageEvent::Done { reason, .. } => done_reason = Some(reason),
                _ => {}
            }
        }
        assert_eq!(text, "hi there");
        assert_eq!(tool_name.as_deref(), Some("weather"));
        assert_eq!(done_reason, Some(DoneReason::ToolUse));
    }

    #[tokio::test]
    async fn falls_back_to_canned_message() {
        let _guard = test_lock();
        clear_faux_responses();
        let provider = FauxProvider::default();
        let msg = provider
            .stream(&faux_model(), &Context::default(), None)
            .result()
            .await;
        assert!(msg.is_some());
    }
}
