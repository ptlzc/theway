//! Bedrock-Anthropic streaming adapter. Each event-stream payload Bedrock emits for Claude
//! models contains a base64-encoded JSON object that mirrors Anthropic's own SSE event
//! shape (`message_start` / `content_block_start` / `content_block_delta` /
//! `content_block_stop` / `message_delta` / `message_stop`). This module converts that
//! payload stream into theway-llm-provider's `AssistantMessageEvent` stream.
//!
//! Wired downstream of [`crate::bedrock_provider::invoke_stream`].

#![allow(dead_code)]

use base64::Engine;
use serde::Deserialize;

use crate::event_stream::EventMessage;
use crate::types::{
    AssistantMessage, AssistantMessageEvent, AssistantRole, ContentBlock, DoneReason, ErrorReason,
    StopReason, TextContent, ThinkingContent, ToolCall, Usage,
};

/// Stateful converter. Keeps the running `AssistantMessage` so each yielded event carries the
/// partial-as-of-this-moment snapshot theway-llm-provider consumers expect.
pub struct Converter {
    msg: AssistantMessage,
    current_index: usize,
    started: bool,
}

impl Default for Converter {
    fn default() -> Self {
        Self::new()
    }
}

impl Converter {
    pub fn new() -> Self {
        Self {
            msg: AssistantMessage {
                role: AssistantRole::Assistant,
                content: Vec::new(),
                api: crate::types::Api::from("bedrock-anthropic"),
                provider: crate::types::Provider::from("amazon-bedrock"),
                model: String::new(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            },
            current_index: 0,
            started: false,
        }
    }

    /// Feed one event-stream EventMessage. Returns zero or more typed events. Returning
    /// `Ok(vec![])` is normal — many incoming frames don't need to be surfaced (e.g.
    /// `message_start` is consumed internally to bootstrap state).
    pub fn ingest(&mut self, msg: &EventMessage) -> Result<Vec<AssistantMessageEvent>, String> {
        // Bedrock-Anthropic puts the JSON payload under the "bytes" field, base64-encoded.
        let payload: serde_json::Value = serde_json::from_slice(&msg.payload)
            .map_err(|e| format!("bedrock chunk not JSON: {e}"))?;
        let b64 = payload
            .get("bytes")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "bedrock chunk missing `bytes`".to_string())?;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("bedrock chunk b64 decode: {e}"))?;
        let evt: AnthropicStreamEvent =
            serde_json::from_slice(&raw).map_err(|e| format!("anthropic event parse: {e}"))?;
        self.handle(evt)
    }

    fn handle(&mut self, evt: AnthropicStreamEvent) -> Result<Vec<AssistantMessageEvent>, String> {
        let mut out = Vec::new();
        match evt {
            AnthropicStreamEvent::MessageStart { message } => {
                self.msg.model = message.model.unwrap_or_default();
                self.msg.response_id = message.id;
                if let Some(role) = message.role {
                    self.msg.role = match role.as_str() {
                        "assistant" => AssistantRole::Assistant,
                        _ => AssistantRole::Assistant,
                    };
                }
                self.started = true;
                out.push(AssistantMessageEvent::Start {
                    partial: self.msg.clone(),
                });
            }
            AnthropicStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.current_index = index;
                // Pad / set the content slot.
                while self.msg.content.len() <= index {
                    self.msg.content.push(ContentBlock::text(""));
                }
                match content_block.r#type.as_str() {
                    "text" => {
                        self.msg.content[index] = ContentBlock::Text(TextContent {
                            text: content_block.text.unwrap_or_default(),
                            text_signature: None,
                        });
                        out.push(AssistantMessageEvent::TextStart {
                            content_index: index,
                            partial: self.msg.clone(),
                        });
                    }
                    "thinking" => {
                        self.msg.content[index] = ContentBlock::Thinking(ThinkingContent {
                            thinking: content_block.thinking.unwrap_or_default(),
                            thinking_signature: None,
                            redacted: false,
                        });
                        out.push(AssistantMessageEvent::ThinkingStart {
                            content_index: index,
                            partial: self.msg.clone(),
                        });
                    }
                    "tool_use" => {
                        self.msg.content[index] = ContentBlock::ToolCall(ToolCall {
                            id: content_block.id.unwrap_or_default(),
                            name: content_block.name.unwrap_or_default(),
                            arguments: serde_json::Map::new(),
                            thought_signature: None,
                        });
                        out.push(AssistantMessageEvent::ToolCallStart {
                            content_index: index,
                            partial: self.msg.clone(),
                        });
                    }
                    _ => {} // ignore unknown
                }
            }
            AnthropicStreamEvent::ContentBlockDelta { index, delta } => {
                while self.msg.content.len() <= index {
                    self.msg.content.push(ContentBlock::text(""));
                }
                match delta.r#type.as_str() {
                    "text_delta" => {
                        let text = delta.text.unwrap_or_default();
                        if let ContentBlock::Text(t) = &mut self.msg.content[index] {
                            t.text.push_str(&text);
                        }
                        out.push(AssistantMessageEvent::TextDelta {
                            content_index: index,
                            delta: text,
                            partial: self.msg.clone(),
                        });
                    }
                    "thinking_delta" => {
                        let thought = delta.thinking.unwrap_or_default();
                        if let ContentBlock::Thinking(t) = &mut self.msg.content[index] {
                            t.thinking.push_str(&thought);
                        }
                        out.push(AssistantMessageEvent::ThinkingDelta {
                            content_index: index,
                            delta: thought,
                            partial: self.msg.clone(),
                        });
                    }
                    "input_json_delta" => {
                        let json = delta.partial_json.unwrap_or_default();
                        out.push(AssistantMessageEvent::ToolCallDelta {
                            content_index: index,
                            delta: json,
                            partial: self.msg.clone(),
                        });
                    }
                    _ => {}
                }
            }
            AnthropicStreamEvent::ContentBlockStop { index } => {
                if let Some(block) = self.msg.content.get(index).cloned() {
                    match block {
                        ContentBlock::Text(t) => {
                            out.push(AssistantMessageEvent::TextEnd {
                                content_index: index,
                                content: t.text,
                                partial: self.msg.clone(),
                            });
                        }
                        ContentBlock::Thinking(t) => {
                            out.push(AssistantMessageEvent::ThinkingEnd {
                                content_index: index,
                                content: t.thinking,
                                partial: self.msg.clone(),
                            });
                        }
                        ContentBlock::ToolCall(c) => {
                            out.push(AssistantMessageEvent::ToolCallEnd {
                                content_index: index,
                                tool_call: c,
                                partial: self.msg.clone(),
                            });
                        }
                        ContentBlock::Image(_) => {}
                    }
                }
            }
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                if let Some(reason) = delta.stop_reason {
                    self.msg.stop_reason = match reason.as_str() {
                        "end_turn" | "stop_sequence" => StopReason::Stop,
                        "tool_use" => StopReason::ToolUse,
                        "max_tokens" => StopReason::Length,
                        _ => StopReason::Stop,
                    };
                }
                if let Some(u) = usage {
                    self.msg.usage.input = u.input_tokens.unwrap_or(0);
                    self.msg.usage.output = u.output_tokens.unwrap_or(0);
                    self.msg.usage.cache_read = u.cache_read_input_tokens.unwrap_or(0);
                    self.msg.usage.cache_write = u.cache_creation_input_tokens.unwrap_or(0);
                    self.msg.usage.total_tokens = self.msg.usage.input + self.msg.usage.output;
                }
            }
            AnthropicStreamEvent::MessageStop {} => {
                let reason = match self.msg.stop_reason {
                    StopReason::ToolUse => DoneReason::ToolUse,
                    StopReason::Length => DoneReason::Length,
                    _ => DoneReason::Stop,
                };
                out.push(AssistantMessageEvent::Done {
                    reason,
                    message: self.msg.clone(),
                });
            }
            AnthropicStreamEvent::Error { error } => {
                self.msg.error_message = Some(error.message.clone());
                self.msg.stop_reason = StopReason::Error;
                out.push(AssistantMessageEvent::Error {
                    reason: ErrorReason::Error,
                    error: self.msg.clone(),
                });
            }
            AnthropicStreamEvent::Ping {} => {}
        }
        Ok(out)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum AnthropicStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessageHead },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: AnthropicContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: AnthropicDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: AnthropicMessageDelta,
        #[serde(default)]
        usage: Option<AnthropicUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop {},
    #[serde(rename = "error")]
    Error { error: AnthropicErrorBody },
    #[serde(rename = "ping")]
    Ping {},
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageHead {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorBody {
    #[serde(default)]
    message: String,
}

#[cfg(test)]
// Test files live in `tests/bedrock_anthropic/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
#[path = "../tests/bedrock_anthropic/mod.rs"]
mod tests;
