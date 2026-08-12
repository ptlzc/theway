//! Shared helpers between `google` (Gemini API) and `google_vertex`. Partial 1:1 port of
//! `packages/ai/src/providers/google-shared.ts`.
//!
//! Implemented: message conversion (contents/parts), tool conversion (functionDeclarations),
//! stop-reason mapping, thought-signature retention.
//! TODO: multimodal functionResponse parts, tool-choice config mode, gemma/gemini3 thinking.

use serde_json::{Value, json};

use crate::types::*;

/// A Gemini "thought" part carries `thought: true`.
pub fn is_thinking_part(part: &Value) -> bool {
    part.get("thought")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Keep a non-empty incoming signature, else retain the existing one. Never merges.
pub fn retain_thought_signature(
    existing: Option<String>,
    incoming: Option<&str>,
) -> Option<String> {
    match incoming {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ => existing,
    }
}

pub fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" => StopReason::Stop,
        "MAX_TOKENS" => StopReason::Length,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" => StopReason::Error,
        _ => StopReason::Stop,
    }
}

/// Convert pi messages into Gemini `contents`. The system prompt is handled separately by the
/// provider (as `systemInstruction`), so it is not included here.
pub fn convert_messages(msgs: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::with_capacity(msgs.len());
    for m in msgs {
        match m {
            Message::User(u) => {
                let parts = match &u.content {
                    UserContent::Text(s) => vec![json!({ "text": s })],
                    UserContent::Blocks(blocks) => blocks.iter().map(user_block_to_part).collect(),
                };
                if parts.is_empty() {
                    continue;
                }
                out.push(json!({ "role": "user", "parts": parts }));
            }
            Message::Assistant(a) => {
                let mut parts = Vec::new();
                for b in &a.content {
                    match b {
                        ContentBlock::Text(t) => {
                            let mut p = json!({ "text": t.text });
                            if let Some(sig) = &t.text_signature {
                                p["thoughtSignature"] = json!(sig);
                            }
                            parts.push(p);
                        }
                        ContentBlock::Thinking(t) => {
                            let mut p = json!({ "text": t.thinking, "thought": true });
                            if let Some(sig) = &t.thinking_signature {
                                p["thoughtSignature"] = json!(sig);
                            }
                            parts.push(p);
                        }
                        ContentBlock::ToolCall(tc) => {
                            let mut p = json!({
                                "functionCall": { "name": tc.name, "args": tc.arguments },
                            });
                            if let Some(sig) = &tc.thought_signature {
                                p["thoughtSignature"] = json!(sig);
                            }
                            parts.push(p);
                        }
                        ContentBlock::Image(_) => {}
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                out.push(json!({ "role": "model", "parts": parts }));
            }
            Message::ToolResult(tr) => {
                let text: String = tr
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        UserContentBlock::Text(t) => Some(t.text.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let part = json!({
                    "functionResponse": {
                        "name": tr.tool_name,
                        "response": { "result": text },
                    },
                });
                // Gemini groups consecutive functionResponses into one user content.
                if let Some(last) = out.last_mut() {
                    if last["role"] == "user"
                        && last["parts"]
                            .as_array()
                            .is_some_and(|p| p.iter().any(|x| x.get("functionResponse").is_some()))
                    {
                        last["parts"].as_array_mut().unwrap().push(part);
                        continue;
                    }
                }
                out.push(json!({ "role": "user", "parts": [part] }));
            }
        }
    }
    out
}

fn user_block_to_part(b: &UserContentBlock) -> Value {
    match b {
        UserContentBlock::Text(t) => json!({ "text": t.text }),
        UserContentBlock::Image(i) => json!({
            "inlineData": { "mimeType": i.mime_type, "data": i.data },
        }),
    }
}

/// Convert pi tools to a single Gemini `tools` entry with `functionDeclarations`.
pub fn convert_tools(tools: &[Tool]) -> Vec<Value> {
    let decls: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();
    vec![json!({ "functionDeclarations": decls })]
}
