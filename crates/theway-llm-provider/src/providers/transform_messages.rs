//! Cross-provider message-history rewriter. 1:1 port of
//! `packages/ai/src/providers/transform-messages.ts`.
//!
//! When a session started on one provider hands off to another, the saved history must be
//! rewritten into the destination provider's expected shape:
//! - downgrade images to placeholders when the target model has no vision input
//! - drop redacted thinking cross-model; convert plain thinking to text cross-model
//! - strip `thought_signature` cross-model; normalise tool-call ids
//! - skip errored/aborted assistant turns
//! - synthesize empty tool results for orphaned tool calls

use std::collections::{HashMap, HashSet};

use crate::types::*;

const NON_VISION_USER_IMAGE_PLACEHOLDER: &str = "(image omitted: model does not support images)";
const NON_VISION_TOOL_IMAGE_PLACEHOLDER: &str =
    "(tool image omitted: model does not support images)";

/// Callback to normalise a tool-call id for the destination provider.
pub type ToolCallIdNormalizer<'a> = dyn Fn(&str, &Model, &AssistantMessage) -> String + 'a;

fn replace_images_with_placeholder(
    content: &[UserContentBlock],
    placeholder: &str,
) -> Vec<UserContentBlock> {
    let mut result = Vec::with_capacity(content.len());
    let mut previous_was_placeholder = false;
    for block in content {
        match block {
            UserContentBlock::Image(_) => {
                if !previous_was_placeholder {
                    result.push(UserContentBlock::text(placeholder));
                }
                previous_was_placeholder = true;
            }
            UserContentBlock::Text(t) => {
                previous_was_placeholder = t.text == placeholder;
                result.push(UserContentBlock::Text(t.clone()));
            }
        }
    }
    result
}

fn downgrade_unsupported_images(messages: Vec<Message>, model: &Model) -> Vec<Message> {
    if model.input.contains(&InputModality::Image) {
        return messages;
    }
    messages
        .into_iter()
        .map(|msg| match msg {
            Message::User(mut u) => {
                if let UserContent::Blocks(blocks) = &u.content {
                    u.content = UserContent::Blocks(replace_images_with_placeholder(
                        blocks,
                        NON_VISION_USER_IMAGE_PLACEHOLDER,
                    ));
                }
                Message::User(u)
            }
            Message::ToolResult(mut tr) => {
                tr.content =
                    replace_images_with_placeholder(&tr.content, NON_VISION_TOOL_IMAGE_PLACEHOLDER);
                Message::ToolResult(tr)
            }
            other => other,
        })
        .collect()
}

/// Rewrite `messages` for the destination `model`.
pub fn transform_messages(
    messages: Vec<Message>,
    model: &Model,
    normalize_tool_call_id: Option<&ToolCallIdNormalizer>,
) -> Vec<Message> {
    let mut tool_call_id_map: HashMap<String, String> = HashMap::new();
    let image_aware = downgrade_unsupported_images(messages, model);

    // First pass: per-message transforms.
    let transformed: Vec<Message> = image_aware
        .into_iter()
        .map(|msg| match msg {
            Message::User(u) => Message::User(u),
            Message::ToolResult(mut tr) => {
                if let Some(norm) = tool_call_id_map.get(&tr.tool_call_id) {
                    if norm != &tr.tool_call_id {
                        tr.tool_call_id = norm.clone();
                    }
                }
                Message::ToolResult(tr)
            }
            Message::Assistant(a) => {
                let is_same_model =
                    a.provider == model.provider && a.api == model.api && a.model == model.id;
                let mut new_content = Vec::with_capacity(a.content.len());
                for block in &a.content {
                    match block {
                        ContentBlock::Thinking(t) => {
                            if t.redacted {
                                if is_same_model {
                                    new_content.push(block.clone());
                                }
                                continue;
                            }
                            if is_same_model && t.thinking_signature.is_some() {
                                new_content.push(block.clone());
                                continue;
                            }
                            if t.thinking.trim().is_empty() {
                                continue;
                            }
                            if is_same_model {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(ContentBlock::text(t.thinking.clone()));
                            }
                        }
                        ContentBlock::Text(t) => {
                            if is_same_model {
                                new_content.push(block.clone());
                            } else {
                                new_content.push(ContentBlock::text(t.text.clone()));
                            }
                        }
                        ContentBlock::ToolCall(tc) => {
                            let mut normalized = tc.clone();
                            if !is_same_model && normalized.thought_signature.is_some() {
                                normalized.thought_signature = None;
                            }
                            if !is_same_model {
                                if let Some(norm) = normalize_tool_call_id {
                                    let new_id = norm(&tc.id, model, &a);
                                    if new_id != tc.id {
                                        tool_call_id_map.insert(tc.id.clone(), new_id.clone());
                                        normalized.id = new_id;
                                    }
                                }
                            }
                            new_content.push(ContentBlock::ToolCall(normalized));
                        }
                        ContentBlock::Image(_) => new_content.push(block.clone()),
                    }
                }
                Message::Assistant(AssistantMessage {
                    content: new_content,
                    ..a
                })
            }
        })
        .collect();

    // Second pass: synthesize empty tool results for orphaned tool calls.
    let mut result: Vec<Message> = Vec::with_capacity(transformed.len());
    let mut pending: Vec<ToolCall> = Vec::new();
    let mut existing_ids: HashSet<String> = HashSet::new();

    fn flush(
        result: &mut Vec<Message>,
        pending: &mut Vec<ToolCall>,
        existing_ids: &mut HashSet<String>,
    ) {
        if pending.is_empty() {
            return;
        }
        for tc in pending.drain(..) {
            if !existing_ids.contains(&tc.id) {
                result.push(Message::ToolResult(ToolResultMessage {
                    role: ToolResultRole::ToolResult,
                    tool_call_id: tc.id,
                    tool_name: tc.name,
                    content: vec![UserContentBlock::text("No result provided")],
                    details: None,
                    is_error: true,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }));
            }
        }
        existing_ids.clear();
    }

    for msg in transformed {
        match msg {
            Message::Assistant(a) => {
                flush(&mut result, &mut pending, &mut existing_ids);
                // Skip errored/aborted assistant turns — incomplete, must not be replayed.
                if matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    continue;
                }
                let tool_calls: Vec<ToolCall> = a
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolCall(tc) => Some(tc.clone()),
                        _ => None,
                    })
                    .collect();
                if !tool_calls.is_empty() {
                    pending = tool_calls;
                    existing_ids = HashSet::new();
                }
                result.push(Message::Assistant(a));
            }
            Message::ToolResult(tr) => {
                existing_ids.insert(tr.tool_call_id.clone());
                result.push(Message::ToolResult(tr));
            }
            Message::User(u) => {
                flush(&mut result, &mut pending, &mut existing_ids);
                result.push(Message::User(u));
            }
        }
    }
    flush(&mut result, &mut pending, &mut existing_ids);

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_model() -> Model {
        Model {
            id: "claude-x".into(),
            name: "Claude X".into(),
            api: Api::known(KnownApi::AnthropicMessages),
            provider: Provider::from("anthropic"),
            base_url: String::new(),
            reasoning: true,
            thinking_level_map: None,
            input: vec![InputModality::Text], // no image support
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    fn assistant_from(provider: &str, content: Vec<ContentBlock>, stop: StopReason) -> Message {
        Message::Assistant(AssistantMessage {
            role: AssistantRole::Assistant,
            content,
            api: Api::known(KnownApi::OpenAIResponses),
            provider: Provider::from(provider),
            model: "gpt".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: stop,
            error_message: None,
            timestamp: 0,
        })
    }

    #[test]
    fn cross_model_thinking_becomes_text() {
        let msgs = vec![assistant_from(
            "openai",
            vec![ContentBlock::Thinking(ThinkingContent {
                thinking: "let me think".into(),
                thinking_signature: None,
                redacted: false,
            })],
            StopReason::Stop,
        )];
        let out = transform_messages(msgs, &target_model(), None);
        if let Message::Assistant(a) = &out[0] {
            assert!(matches!(a.content[0], ContentBlock::Text(_)));
        } else {
            panic!("expected assistant");
        }
    }

    #[test]
    fn errored_assistant_is_skipped() {
        let msgs = vec![
            assistant_from(
                "openai",
                vec![ContentBlock::text("partial")],
                StopReason::Error,
            ),
            Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("next".into()),
                timestamp: 0,
            }),
        ];
        let out = transform_messages(msgs, &target_model(), None);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Message::User(_)));
    }

    #[test]
    fn orphaned_tool_call_gets_synthetic_result() {
        let mut args = serde_json::Map::new();
        args.insert("x".into(), serde_json::json!(1));
        let msgs = vec![
            assistant_from(
                "anthropic",
                vec![ContentBlock::ToolCall(ToolCall {
                    id: "call_1".into(),
                    name: "tool".into(),
                    arguments: args,
                    thought_signature: None,
                })],
                StopReason::ToolUse,
            ),
            Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("interrupt".into()),
                timestamp: 0,
            }),
        ];
        let out = transform_messages(msgs, &target_model(), None);
        // assistant, synthetic toolResult, user
        assert_eq!(out.len(), 3);
        assert!(
            matches!(out[1], Message::ToolResult(ref tr) if tr.is_error && tr.tool_call_id == "call_1")
        );
    }

    #[test]
    fn images_downgraded_for_non_vision_model() {
        let msgs = vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Blocks(vec![
                UserContentBlock::text("look at this"),
                UserContentBlock::Image(ImageContent {
                    data: "abc".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            timestamp: 0,
        })];
        let out = transform_messages(msgs, &target_model(), None);
        if let Message::User(u) = &out[0] {
            if let UserContent::Blocks(blocks) = &u.content {
                assert_eq!(blocks.len(), 2);
                assert!(
                    matches!(&blocks[1], UserContentBlock::Text(t) if t.text.contains("image omitted"))
                );
            } else {
                panic!("expected blocks");
            }
        } else {
            panic!("expected user");
        }
    }
}
