//! Extra tests for `agent::compaction::compaction` — bridged through
//! `compaction_extra_tests` because the existing test module was already occupied.

use super::super::*;
use std::sync::Arc;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, ImageContent, Message as PiMessage, StopReason, ToolCall,
    ToolResultMessage, ToolResultRole, Usage, UserContent, UserContentBlock, UserMessage, UserRole,
};

fn user(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant(
    content: Vec<ContentBlock>,
    stop: StopReason,
    usage: Usage,
) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content,
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: stop,
        error_message: None,
        timestamp: 0,
    }))
}

fn tool_result(tool_name: &str, content: Vec<UserContentBlock>) -> AgentMessage {
    AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "t".into(),
        tool_name: tool_name.into(),
        content,
        details: None,
        is_error: false,
        timestamp: 0,
    }))
}

fn message_entry(id: &str, parent_id: Option<&str>, message: AgentMessage) -> SessionTreeEntry {
    SessionTreeEntry::Message {
        id: id.into(),
        parent_id: parent_id.map(str::to_string),
        timestamp: "t".into(),
        message,
    }
}

fn faux_model(context_window: u32, max_tokens: u32) -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window,
        max_tokens,
        headers: None,
        compat: None,
    }
}

fn default_settings() -> CompactionSettings {
    DEFAULT_COMPACTION_SETTINGS.clone()
}

#[test]
fn calculate_context_tokens_prefers_total_tokens_over_sum() {
    let usage = Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        total_tokens: 100,
        ..Usage::default()
    };
    assert_eq!(calculate_context_tokens(&usage), 100);

    let no_total = Usage {
        input: 1,
        output: 2,
        cache_read: 3,
        cache_write: 4,
        total_tokens: 0,
        ..Usage::default()
    };
    assert_eq!(calculate_context_tokens(&no_total), 10);
}

#[test]
fn get_last_assistant_usage_skips_aborted_and_zero_usage() {
    let entries = vec![
        message_entry("1", None, user("hi")),
        message_entry(
            "2",
            Some("1"),
            assistant(
                vec![ContentBlock::text("aborted")],
                StopReason::Aborted,
                Usage {
                    total_tokens: 10,
                    ..Usage::default()
                },
            ),
        ),
        message_entry(
            "3",
            Some("2"),
            assistant(vec![ContentBlock::text("ok")], StopReason::Stop, Usage::default()),
        ),
        message_entry(
            "4",
            Some("3"),
            assistant(
                vec![ContentBlock::text("real")],
                StopReason::Stop,
                Usage {
                    input: 20,
                    output: 5,
                    total_tokens: 25,
                    ..Usage::default()
                },
            ),
        ),
    ];

    let usage = get_last_assistant_usage(&entries).expect("last assistant usage");
    assert_eq!(usage.total_tokens, 25);
}

#[test]
fn estimate_text_tokens_weights_ascii_and_non_ascii() {
    assert_eq!(estimate_text_tokens(""), 0);
    assert_eq!(estimate_text_tokens("abcd"), 1);
    assert_eq!(estimate_text_tokens("abcde"), 2);
    assert_eq!(estimate_text_tokens("夏夏"), 2);
    assert_eq!(estimate_text_tokens("a夏"), 2);
}

#[test]
fn estimate_tokens_covers_user_assistant_tool_and_custom_messages() {
    let user_blocks = AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Blocks(vec![
            UserContentBlock::text("abcd"),
            UserContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
        ]),
        timestamp: 0,
    }));
    assert_eq!(estimate_tokens(&user_blocks), 1 + 768);

    let assistant = assistant(
        vec![
            ContentBlock::text("abcd"),
            ContentBlock::ToolCall(ToolCall {
                id: "t".into(),
                name: "read".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            }),
        ],
        StopReason::Stop,
        Usage::default(),
    );
    // "abcd" -> 1 token, "read" -> 1 token, "{}" -> 1 token.
    assert_eq!(estimate_tokens(&assistant), 3);

    let tr = tool_result("grep", vec![UserContentBlock::text("abcd")]);
    assert_eq!(estimate_tokens(&tr), 1 + 1);

    let custom = AgentMessage::Custom(crate::types::CustomMessage {
        role: "role".into(),
        timestamp: 0,
        payload: serde_json::json!({"abcd": "x"}),
    });
    assert!(estimate_tokens(&custom) > 0);
}

#[test]
fn estimate_context_tokens_without_usage_falls_back_to_sum() {
    let msgs = vec![user("abcdefgh")];
    let est = estimate_context_tokens(&msgs);
    assert_eq!(est.usage_tokens, 0);
    assert_eq!(est.tokens, 2);
    assert_eq!(est.trailing_tokens, 2);
    assert_eq!(est.last_usage_index, None);
}

#[test]
fn find_turn_start_index_returns_start_when_no_user_message() {
    let entries = vec![
        message_entry("1", None, assistant(vec![ContentBlock::text("a")], StopReason::Stop, Usage::default())),
        message_entry("2", Some("1"), assistant(vec![ContentBlock::text("b")], StopReason::Stop, Usage::default())),
    ];

    assert_eq!(find_turn_start_index(&entries, 1, 0), 0);
    assert_eq!(find_turn_start_index(&entries, 5, 2), 2);
}

#[test]
fn find_cut_point_returns_zero_for_empty_entries() {
    let settings = default_settings();
    let cut = find_cut_point(&[], &settings);
    assert_eq!(cut.cut_index, 0);
    assert_eq!(cut.first_kept_entry_id, None);
}

#[test]
fn serialize_conversation_renders_all_message_kinds() {
    let messages = vec![
        user("hello"),
        assistant(
            vec![
                ContentBlock::text("assistant text"),
                ContentBlock::Thinking(theway_llm_provider::ThinkingContent {
                    thinking: "thinking".into(),
                    thinking_signature: None,
                    redacted: false,
                }),
                ContentBlock::ToolCall(ToolCall {
                    id: "t".into(),
                    name: "read".into(),
                    arguments: {
                        let mut m = serde_json::Map::new();
                        m.insert("path".into(), serde_json::json!("file"));
                        m
                    },
                    thought_signature: None,
                }),
            ],
            StopReason::Stop,
            Usage::default(),
        ),
        tool_result("grep", vec![UserContentBlock::text("match")]),
        AgentMessage::Custom(crate::types::CustomMessage {
            role: "note".into(),
            timestamp: 0,
            payload: serde_json::json!({"k": "v"}),
        }),
    ];

    let text = serialize_conversation(&messages);

    assert!(text.contains("USER:\nhello"));
    assert!(text.contains("ASSISTANT:\n"));
    assert!(text.contains("assistant text"));
    assert!(text.contains("<thinking>thinking</thinking>"));
    assert!(text.contains("<tool_call name=\"read\">"));
    assert!(text.contains("TOOL_RESULT[grep]:\nmatch"));
    assert!(text.contains("NOTE:\n"));
}

#[test]
fn summary_output_tokens_caps_to_reserve_and_context_window() {
    let settings = CompactionSettings {
        reserve_tokens: 16_384,
        ..default_settings()
    };
    assert_eq!(
        summary_output_tokens(&faux_model(200_000, 64_000), &settings),
        16_384
    );
    assert_eq!(
        summary_output_tokens(&faux_model(0, 0), &settings),
        16_384
    );
    assert_eq!(
        summary_output_tokens(&faux_model(8_000, 64_000), &settings),
        2_000
    );
}

#[test]
fn summarization_prompt_budget_returns_default_for_unknown_window() {
    let model = faux_model(0, 0);
    let settings = default_settings();
    assert_eq!(
        summarization_prompt_budget(&model, &settings),
        64_000
    );
}

#[test]
fn summarize_prompt_estimate_tokens_includes_overhead_and_messages() {
    let overhead = summary_prompt_overhead_tokens(Some("extra"));
    assert!(overhead > estimate_text_tokens(SUMMARIZATION_SYSTEM_PROMPT));

    let estimate = summarize_prompt_estimate_tokens(&[user("hello")], Some("extra"));
    assert!(estimate > overhead);
}

#[test]
fn trim_messages_for_summary_budget_keeps_tail_and_discloses_omissions() {
    let messages: Vec<AgentMessage> = (0..10)
        .map(|i| user(&format!("message-{i} {}", "x".repeat(100))))
        .collect();
    let full = summarize_prompt_estimate_tokens(&messages, None);

    let trimmed = trim_messages_for_summary_budget(&messages, full / 2, None);

    assert!(trimmed.len() < messages.len());
    assert!(matches!(
        &trimmed[0],
        AgentMessage::Llm(PiMessage::User(_))
    ));
}

#[test]
fn suffix_start_for_token_budget_respects_cjk_tokens() {
    let s = "a".repeat(4_000) + "夏";
    let start = suffix_start_for_token_budget(&s, 1_000);
    assert!(start > 0);
    let suffix = &s[start..];
    let ascii = suffix.chars().filter(|c| c.is_ascii()).count() as u64;
    let non_ascii = suffix.chars().count() as u64 - ascii;
    assert!(ascii.div_ceil(4) + non_ascii <= 1_000);
}

#[test]
fn prepare_compaction_splits_prefix_and_sums_message_tokens() {
    let entries = vec![
        message_entry("1", None, user("old")),
        message_entry("2", Some("1"), user("keep")),
    ];
    let settings = CompactionSettings {
        keep_recent_tokens: 1,
        ..default_settings()
    };
    let prep = prepare_compaction(&entries, &settings);
    assert_eq!(prep.cut.cut_index, 1);
    assert_eq!(prep.cut.first_kept_entry_id.as_deref(), Some("2"));
    assert_eq!(prep.entries_to_summarize.len(), 1);
    assert!(prep.tokens_before > 0);
}

#[test]
fn summary_output_tokens_falls_back_to_default_reserve_when_zero() {
    let settings = CompactionSettings {
        reserve_tokens: 0,
        ..default_settings()
    };
    assert_eq!(
        summary_output_tokens(&faux_model(0, 0), &settings),
        DEFAULT_COMPACTION_SETTINGS.reserve_tokens
    );
    assert_eq!(summary_output_tokens(&faux_model(4_000, 0), &settings), 1_000);
}

#[test]
fn serialize_conversation_for_summary_budget_smaller_than_note_truncates_note() {
    // Budget below the note overhead: the serialized conversation is empty.
    let text = serialize_conversation_for_summary_budget(&[user("hello")], 1, None);
    assert!(text.is_empty());

    // Budget that fits part of the note: the note is truncated to the
    // char-class token estimate for the available budget.
    let overhead = summary_prompt_overhead_tokens(None);
    let text = serialize_conversation_for_summary_budget(&[user(&"x".repeat(2000))], overhead + 10, None);
    assert!(text.contains("[compaction note"));
    assert!(!text.contains("hello"));
}

#[tokio::test]
async fn generate_summary_with_custom_instructions_and_budget() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<PiContext>));
    let captured_clone = captured.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, options| {
        *captured_clone.lock().unwrap() = Some(context.clone());
        assert!(options.and_then(|o| o.base.max_tokens).is_some());
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![theway_llm_provider::ContentBlock::text("summary")],
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
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });

    let out = generate_summary(
        GenerateSummaryRequest {
            model: faux_model(128_000, 16_384),
            messages: vec![user("hello")],
            custom_instructions: Some("custom instructions".into()),
            prompt_budget_tokens: Some(8_000),
            max_output_tokens: Some(1_024),
            stream_fn: Some(stream_fn),
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(out.summary, "summary");
    let ctx = captured.lock().unwrap().clone().unwrap();
    let system = ctx.system_prompt.expect("system prompt set");
    assert!(system.contains("custom instructions"));
    assert!(system.contains(SUMMARIZATION_SYSTEM_PROMPT));
}
