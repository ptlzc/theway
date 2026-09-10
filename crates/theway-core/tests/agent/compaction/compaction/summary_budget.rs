//! Summarizer budget and projection: conversation serialisation caps, CJK
//! token accounting, tool-output projection, usage extraction, trimming.

use super::*;
use theway_core::agent::compaction::estimate::get_last_assistant_usage;

#[test]
fn summary_budget_caps_single_oversized_message() {
    let conversation =
        serialize_conversation_for_summary_budget(&[user(&"x".repeat(50_000))], 2_000, None);
    assert!(
        conversation.len().div_ceil(4) <= 2_000,
        "serialized compaction prompt must fit the budget; got {} chars",
        conversation.len()
    );
    assert!(
        conversation.starts_with("[compaction note: omitted older serialized content"),
        "single-message truncation must disclose omitted content"
    );
}

#[test]
fn summary_budget_leaves_room_for_output_and_estimate_error() {
    let model = model_with_limits(200_000, 64_000);
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 16_384,
        keep_recent_tokens: 20_000,
        algorithm: default_compaction_algorithm(),
    };
    let budget = summarization_prompt_budget(&model, &settings);
    assert!(budget > 0);
    // The char-based token estimate can undercount by ~20-30% on code or CJK text, so the
    // prompt budget must keep slack below (window - reserved output) rather than using it all.
    assert!(
        budget <= (200_000 - 16_384) * 4 / 5,
        "budget {budget} leaves no slack for token-estimate error"
    );
}

#[test]
fn cjk_truncation_respects_token_budget() {
    // CJK chars are ~1 token each but 3 UTF-8 bytes; a bytes/4 estimate undercounts ~3x.
    let conversation =
        serialize_conversation_for_summary_budget(&[user(&"夏".repeat(50_000))], 2_000, None);
    let ascii = conversation.chars().filter(char::is_ascii).count() as u64;
    let non_ascii = conversation.chars().count() as u64 - ascii;
    let estimated_tokens = ascii.div_ceil(4) + non_ascii;
    assert!(
        estimated_tokens <= 2_000,
        "CJK-heavy prompt must fit the token budget; estimated {estimated_tokens} tokens"
    );
    assert!(
        conversation.contains("[compaction note: omitted"),
        "truncation must disclose omitted content"
    );
}

/// Issue #101: the summarization projection caps tool-result bodies, tool-call
/// arguments and thinking, drops `details`/custom self-summaries, and bounds
/// the projected total so the raw event tree never reaches the summarizer.
#[test]
fn projection_caps_tool_outputs_and_bounds_total() {
    use theway_core::agent::compaction::compaction::{
        SUMMARY_PROJECTED_BUDGET_TOKENS, SUMMARY_TEXT_CAP, SUMMARY_TOOL_CAP,
        project_summary_messages,
    };

    let huge = "y".repeat(20_000);
    let mut entries = Vec::new();
    // A custom compaction summary must be skipped (no self-referencing).
    entries.push(SessionTreeEntry::Message {
        id: "c1".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: AgentMessage::Custom(theway_core::types::CustomMessage {
            role: "compactionSummary".into(),
            timestamp: 0,
            payload: serde_json::json!({"summary": huge.clone()}),
        }),
    });
    // A tool result with a giant body.
    entries.push(SessionTreeEntry::Message {
        id: "m1".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: AgentMessage::Llm(PiMessage::ToolResult(
            theway_llm_provider::ToolResultMessage {
                role: theway_llm_provider::ToolResultRole::ToolResult,
                tool_call_id: "tc1".into(),
                tool_name: "read".into(),
                content: vec![theway_llm_provider::UserContentBlock::text(huge.clone())],
                details: Some(serde_json::json!({"raw": huge.clone()})),
                is_error: false,
                timestamp: 0,
            },
        )),
    });
    // An assistant message with oversized text, thinking and tool-call args.
    let mut args = serde_json::Map::new();
    args.insert("file".into(), serde_json::Value::String(huge.clone()));
    entries.push(SessionTreeEntry::Message {
        id: "m2".into(),
        parent_id: None,
        timestamp: "t".into(),
        message: AgentMessage::Llm(PiMessage::Assistant(
            theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![
                    theway_llm_provider::ContentBlock::text(huge.clone()),
                    theway_llm_provider::ContentBlock::Thinking(
                        theway_llm_provider::ThinkingContent {
                            thinking: huge.clone(),
                            thinking_signature: None,
                            redacted: false,
                        },
                    ),
                    theway_llm_provider::ContentBlock::ToolCall(
                        theway_llm_provider::ToolCall {
                            id: "tc".into(),
                            name: "edit".into(),
                            arguments: args,
                            thought_signature: None,
                        },
                    ),
                ],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: theway_llm_provider::StopReason::Stop,
                error_message: None,
                timestamp: 0,
            },
        )),
    });

    let projected = project_summary_messages(&entries);
    assert_eq!(projected.len(), 2, "custom summary must be skipped");

    let AgentMessage::Llm(PiMessage::ToolResult(tr)) = &projected[0] else {
        panic!("first projected message must be the tool result");
    };
    let text = match &tr.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => &t.text,
        other => panic!("unexpected block {other:?}"),
    };
    assert!(
        text.chars().count() <= SUMMARY_TOOL_CAP + 64,
        "tool body must be capped (got {} chars)",
        text.chars().count()
    );
    assert!(tr.details.is_none(), "details payload must be dropped");

    let AgentMessage::Llm(PiMessage::Assistant(a)) = &projected[1] else {
        panic!("second projected message must be the assistant message");
    };
    let mut saw_text = false;
    let mut saw_thinking = false;
    let mut saw_tool_call = false;
    for block in &a.content {
        match block {
            theway_llm_provider::ContentBlock::Text(t) => {
                saw_text = true;
                assert!(
                    t.text.chars().count() <= SUMMARY_TEXT_CAP + 64,
                    "assistant text must be capped (got {})",
                    t.text.chars().count()
                );
            }
            theway_llm_provider::ContentBlock::Thinking(t) => {
                saw_thinking = true;
                assert!(
                    t.thinking.chars().count() <= SUMMARY_THINKING_CAP + 64,
                    "thinking must be capped (got {})",
                    t.thinking.chars().count()
                );
            }
            theway_llm_provider::ContentBlock::ToolCall(c) => {
                saw_tool_call = true;
                let serialized = serde_json::to_string(&c.arguments).unwrap_or_default();
                assert!(
                    serialized.chars().count() <= SUMMARY_TOOL_CAP + 128,
                    "tool-call arguments must be capped (got {})",
                    serialized.chars().count()
                );
            }
            theway_llm_provider::ContentBlock::Image(_) => {}
        }
    }
    assert!(saw_text && saw_thinking && saw_tool_call);

    // Budget: enough oversized messages drop the oldest projected entries but
    // keep the newest one.
    let mut many = Vec::new();
    let mut parent_id = None;
    for i in 0..60 {
        let id = format!("b-{i}");
        many.push(SessionTreeEntry::Message {
            id: id.clone(),
            parent_id: parent_id.clone(),
            timestamp: "t".into(),
            message: user(&format!("bulk-{i} {}", "z".repeat(8_000))),
        });
        parent_id = Some(id);
    }
    let projected = project_summary_messages(&many);
    assert!(projected.len() < many.len(), "oldest must drop on budget");
    assert!(
        projected
            .last()
            .is_some_and(|m| matches!(&m, AgentMessage::Llm(PiMessage::User(u))
                if matches!(&u.content, theway_llm_provider::UserContent::Text(t) if t.starts_with("bulk-59")))),
        "the newest message must survive the budget"
    );
    let total: u64 = projected
        .iter()
        .map(theway_core::agent::compaction::compaction::estimate_tokens)
        .sum();
    assert!(
        total <= SUMMARY_PROJECTED_BUDGET_TOKENS,
        "projected total {total} must fit the budget {SUMMARY_PROJECTED_BUDGET_TOKENS}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Coverage-gap additions: estimate and compaction edge branches
// ──────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn get_last_assistant_usage_skips_non_message_entries() {
    let entries = vec![SessionTreeEntry::Custom {
        id: "c".into(),
        parent_id: None,
        timestamp: "t".into(),
        custom_type: "other".into(),
        data: None,
    }];
    assert!(get_last_assistant_usage(&entries).is_none());
}

#[test]
fn assistant_usage_accepts_any_nonzero_usage_field() {
    let usage = |total_tokens: u64, input: u64, output: u64, cache_read: u64, cache_write: u64| {
        theway_llm_provider::Usage {
            total_tokens,
            input,
            output,
            cache_read,
            cache_write,
            ..Default::default()
        }
    };

    let assistant = |usage: theway_llm_provider::Usage| {
        AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage,
            stop_reason: theway_llm_provider::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        }))
    };

    for usage in [
        usage(0, 1, 0, 0, 0),
        usage(0, 0, 1, 0, 0),
        usage(0, 0, 0, 1, 0),
        usage(0, 0, 0, 0, 1),
    ] {
        let entries = vec![SessionTreeEntry::Message {
            id: "m".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: assistant(usage),
        }];
        assert!(get_last_assistant_usage(&entries).is_some());
    }
}

#[test]
fn serialize_conversation_tool_result_skips_image_blocks() {
    let tool_result = AgentMessage::Llm(PiMessage::ToolResult(
        theway_llm_provider::ToolResultMessage {
            role: theway_llm_provider::ToolResultRole::ToolResult,
            tool_call_id: "t".into(),
            tool_name: "bash".into(),
            content: vec![
                theway_llm_provider::UserContentBlock::text("line"),
                theway_llm_provider::UserContentBlock::Image(
                    theway_llm_provider::ImageContent {
                        data: "base64".into(),
                        mime_type: "image/png".into(),
                    },
                ),
            ],
            details: None,
            is_error: false,
            timestamp: 0,
        },
    ));
    let serialized = serialize_conversation(std::slice::from_ref(&tool_result));
    assert!(serialized.contains("TOOL_RESULT[bash]:"));
    assert!(serialized.contains("line"));
    assert!(!serialized.contains("base64"));
}

#[test]
fn project_summary_message_caps_user_blocks_and_tool_result_image_blocks() {
    let user_blocks = AgentMessage::Llm(PiMessage::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Blocks(vec![
                theway_llm_provider::UserContentBlock::text("hello"),
                theway_llm_provider::UserContentBlock::Image(
                    theway_llm_provider::ImageContent {
                        data: "base64".into(),
                        mime_type: "image/png".into(),
                    },
                ),
            ]),
            timestamp: 0,
        },
    ));
    let projected = project_summary_message(&user_blocks).unwrap();
    match projected {
        AgentMessage::Llm(PiMessage::User(user)) => match &user.content {
            theway_llm_provider::UserContent::Blocks(blocks) => assert_eq!(blocks.len(), 2),
            other => panic!("expected blocks, got {other:?}"),
        },
        other => panic!("expected user message, got {other:?}"),
    }

    let tool_result = AgentMessage::Llm(PiMessage::ToolResult(
        theway_llm_provider::ToolResultMessage {
            role: theway_llm_provider::ToolResultRole::ToolResult,
            tool_call_id: "t".into(),
            tool_name: "bash".into(),
            content: vec![theway_llm_provider::UserContentBlock::Image(
                theway_llm_provider::ImageContent {
                    data: "base64".into(),
                    mime_type: "image/png".into(),
                },
            )],
            details: Some(serde_json::json!({"exitCode": 1})),
            is_error: false,
            timestamp: 0,
        },
    ));
    let projected = project_summary_message(&tool_result).unwrap();
    match projected {
        AgentMessage::Llm(PiMessage::ToolResult(result)) => {
            assert!(result.details.is_none());
            assert!(matches!(&result.content[0], theway_llm_provider::UserContentBlock::Image(_)));
        }
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn project_summary_message_small_tool_call_arguments_are_kept() {
    let assistant = AgentMessage::Llm(PiMessage::Assistant(
        theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![theway_llm_provider::ContentBlock::ToolCall(
                theway_llm_provider::ToolCall {
                    id: "t".into(),
                    name: "read".into(),
                    arguments: serde_json::Map::from_iter([(
                        "file_path".into(),
                        serde_json::Value::String("small".into()),
                    )]),
                    thought_signature: None,
                },
            )],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: theway_llm_provider::Usage::default(),
            stop_reason: theway_llm_provider::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        },
    ));

    let projected = project_summary_message(&assistant).unwrap();
    match projected {
        AgentMessage::Llm(PiMessage::Assistant(a)) => {
            match &a.content[0] {
                theway_llm_provider::ContentBlock::ToolCall(call) => {
                    assert_eq!(
                        call.arguments.get("file_path").and_then(|v| v.as_str()),
                        Some("small")
                    );
                }
                other => panic!("expected tool call, got {other:?}"),
            }
        }
        other => panic!("expected assistant, got {other:?}"),
    }
}

#[test]
fn trim_projected_budget_single_oversized_message_is_kept() {
    let huge = user(&"x".repeat(400_000));
    let messages = vec![huge];
    let trimmed = trim_projected_budget(messages);
    assert_eq!(trimmed.len(), 1);
}
