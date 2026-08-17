//! Tests for `compaction` — split out of src (see docs/rust-test-files.md).

use super::*;
use super::super::algorithm::BuiltinCompactAlgorithm;
use std::sync::{Arc, Mutex};

fn user(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn model_with_limits(context_window: u32, max_tokens: u32) -> Model {
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

fn model_with_context_window(context_window: u32) -> Model {
    model_with_limits(context_window, 0)
}

fn done_message(text: &str) -> AssistantMessage {
    AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content: vec![theway_llm_provider::ContentBlock::text(text)],
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
    }
}

fn oversized_entries(count: usize) -> Vec<SessionTreeEntry> {
    let mut entries = Vec::new();
    let mut parent_id = None;
    for i in 0..count {
        let id = format!("entry-{i}");
        entries.push(SessionTreeEntry::Message {
            id: id.clone(),
            parent_id: parent_id.clone(),
            timestamp: "t".into(),
            message: user(&format!("old-msg-{i} {}", "x".repeat(1600))),
        });
        parent_id = Some(id);
    }
    entries
}

fn assistant(text: &str, stop: theway_llm_provider::StopReason, usage: Usage) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(
        theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![theway_llm_provider::ContentBlock::text(text)],
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
        },
    ))
}

#[test]
fn should_compact_when_over_threshold() {
    let s = CompactionSettings {
        enabled: true,
        reserve_tokens: 1024,
        keep_recent_tokens: 0,
        algorithm: default_compaction_algorithm(),
    };
    // Threshold is 80% of window = 102_400 for a 128K window.
    assert!(should_compact(102_401, 128_000, &s));
    assert!(!should_compact(102_400, 128_000, &s));
    // Also triggers at higher usage.
    assert!(should_compact(127_000, 128_000, &s));
    // Well below threshold does not trigger.
    assert!(!should_compact(80_000, 128_000, &s));
}

#[tokio::test]
async fn compact_trims_summarizer_prompt_before_provider_call() {
    let mut entries = Vec::new();
    let mut parent_id = None;
    for i in 0..80 {
        let id = format!("entry-{i}");
        entries.push(SessionTreeEntry::Message {
            id: id.clone(),
            parent_id: parent_id.clone(),
            timestamp: "t".into(),
            message: user(&format!("old-msg-{i} {}", "x".repeat(1600))),
        });
        parent_id = Some(id);
    }

    let captured = Arc::new(Mutex::new(String::new()));
    let captured_clone = captured.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        let text = match &context.messages[0] {
            PiMessage::User(user) => match &user.content {
                theway_llm_provider::UserContent::Text(text) => text.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        assert!(
            text.len().div_ceil(4) < 4_000,
            "summarizer prompt must be trimmed before provider dispatch; got {} chars",
            text.len()
        );
        assert!(
            text.contains("[compaction note: omitted"),
            "trimmed prompt must disclose omitted older content"
        );
        assert!(
            !text.contains("old-msg-0"),
            "oldest oversized content should not reach the provider prompt"
        );
        *captured_clone.lock().unwrap() = text;

        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![theway_llm_provider::ContentBlock::text("bounded summary")],
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
            };
            sender.push(AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: msg,
            });
        });
        stream
    });

    let result = compact(
        &BuiltinCompactAlgorithm,
        model_with_context_window(5_000),
        &entries,
        &CompactionSettings {
            enabled: true,
            reserve_tokens: 1_000,
            keep_recent_tokens: 1,
            algorithm: default_compaction_algorithm(),
        },
        None,
        Some(stream_fn),
        CancellationToken::new(),
    )
    .await
    .expect("compaction should succeed with a bounded summarizer prompt");

    assert_eq!(result.summary, "bounded summary");
    assert!(!captured.lock().unwrap().is_empty());
}

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

#[tokio::test]
async fn summarizer_request_sets_bounded_max_tokens() {
    // Claude-4.x shape: 200k window, 64k default max output. The provider falls back to
    // model.max_tokens when options don't set one, which would make input+output overflow
    // the window. The summarizer must send an explicit, bounded max_tokens.
    let entries = oversized_entries(10);
    let captured_max_tokens = Arc::new(Mutex::new(None::<u32>));
    let captured_clone = captured_max_tokens.clone();
    let stream_fn: StreamFn = Arc::new(move |_, _, options| {
        *captured_clone.lock().unwrap() = options.and_then(|o| o.base.max_tokens);
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            sender.push(AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: done_message("summary"),
            });
        });
        stream
    });

    compact(
        &BuiltinCompactAlgorithm,
        model_with_limits(200_000, 64_000),
        &entries,
        &CompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 1,
            algorithm: default_compaction_algorithm(),
        },
        None,
        Some(stream_fn),
        CancellationToken::new(),
    )
    .await
    .expect("compaction should succeed");

    let max_tokens = captured_max_tokens.lock().unwrap().take();
    assert_eq!(
        max_tokens,
        Some(16_384),
        "summarizer must cap output at reserve_tokens instead of inheriting model.max_tokens"
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

#[tokio::test]
async fn compact_retries_with_smaller_budget_on_provider_overflow() {
    // Even a bounded estimate can undercount real tokens; when the provider still rejects
    // the summarizer call as context overflow, compaction must retry with a smaller prompt
    // instead of failing the whole compaction.
    let entries = oversized_entries(80);
    let prompt_lens = Arc::new(Mutex::new(Vec::<usize>::new()));
    let prompt_lens_clone = prompt_lens.clone();
    let stream_fn: StreamFn = Arc::new(move |_, context, _| {
        let text = match &context.messages[0] {
            PiMessage::User(user) => match &user.content {
                theway_llm_provider::UserContent::Text(text) => text.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let call_index = {
            let mut lens = prompt_lens_clone.lock().unwrap();
            lens.push(text.len());
            lens.len()
        };
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            if call_index == 1 {
                let mut error = done_message("");
                error.stop_reason = theway_llm_provider::StopReason::Error;
                error.error_message = Some("prompt is too long: 5500 tokens > 5000 maximum".into());
                sender.push(AssistantMessageEvent::Error {
                    reason: theway_llm_provider::ErrorReason::Error,
                    error,
                });
            } else {
                sender.push(AssistantMessageEvent::Done {
                    reason: theway_llm_provider::DoneReason::Stop,
                    message: done_message("summary after retry"),
                });
            }
        });
        stream
    });

    let result = compact(
        &BuiltinCompactAlgorithm,
        model_with_context_window(5_000),
        &entries,
        &CompactionSettings {
            enabled: true,
            reserve_tokens: 1_000,
            keep_recent_tokens: 1,
            algorithm: default_compaction_algorithm(),
        },
        None,
        Some(stream_fn),
        CancellationToken::new(),
    )
    .await
    .expect("compaction should survive one provider overflow rejection");

    assert_eq!(result.summary, "summary after retry");
    let lens = prompt_lens.lock().unwrap();
    assert_eq!(lens.len(), 2, "expected exactly one retry");
    assert!(
        lens[1] < lens[0],
        "retry must shrink the prompt: {} -> {}",
        lens[0],
        lens[1]
    );
}

#[test]
fn disabled_compaction_returns_false() {
    let s = CompactionSettings {
        enabled: false,
        ..Default::default()
    };
    assert!(!should_compact(1_000_000, 128_000, &s));
}

#[test]
fn estimate_context_tokens_uses_last_usage_block() {
    let msgs = vec![
        user("hi"),
        assistant(
            "ok",
            theway_llm_provider::StopReason::Stop,
            Usage {
                input: 100,
                output: 50,
                total_tokens: 150,
                ..Default::default()
            },
        ),
        user("more"),
    ];
    let est = estimate_context_tokens(&msgs);
    assert_eq!(est.usage_tokens, 150);
    // Trailing user("more") gets char-estimated, so total > 150.
    assert!(est.tokens > 150);
    assert_eq!(est.last_usage_index, Some(1));
}

#[test]
fn cut_point_lands_on_turn_boundary() {
    // entries: U A U A U  (4 turns). Set keep_recent_tokens to a small value so cut is far
    // back; verify it lands on a turn start (user message).
    let entries = vec![
        SessionTreeEntry::Message {
            id: "1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: user("a"),
        },
        SessionTreeEntry::Message {
            id: "2".into(),
            parent_id: Some("1".into()),
            timestamp: "t".into(),
            message: assistant("b", theway_llm_provider::StopReason::Stop, Usage::default()),
        },
        SessionTreeEntry::Message {
            id: "3".into(),
            parent_id: Some("2".into()),
            timestamp: "t".into(),
            message: user("c"),
        },
        SessionTreeEntry::Message {
            id: "4".into(),
            parent_id: Some("3".into()),
            timestamp: "t".into(),
            message: assistant("d", theway_llm_provider::StopReason::Stop, Usage::default()),
        },
    ];
    let cut = find_cut_point(
        &entries,
        &CompactionSettings {
            keep_recent_tokens: 1,
            ..Default::default()
        },
    );
    // Should land on a turn boundary, i.e., a user message or 0.
    if cut.cut_index < entries.len() {
        if let SessionTreeEntry::Message { message, .. } = &entries[cut.cut_index] {
            assert!(matches!(message, AgentMessage::Llm(PiMessage::User(_))) || cut.cut_index == 0);
        }
    }
}
