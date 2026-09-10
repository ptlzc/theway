//! Running compaction against a provider: summarizer prompt trimming, bounded
//! `max_tokens`, overflow retry, and budget-floor overflow reporting.

use super::*;

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

#[tokio::test]
async fn summarize_with_llm_budget_floor_reports_context_overflow() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        let error = theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: theway_llm_provider::Usage::default(),
            stop_reason: theway_llm_provider::StopReason::Error,
            error_message: Some("prompt is too long".into()),
            timestamp: 0,
        };
        sender.push(theway_llm_provider::AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error,
        });
        stream
    });
    let request = SummarizeRequest {
        model: &model_with_limits(1_300, 0),
        messages: &[user("hi")],
        custom_instructions: None,
        settings: &DEFAULT_COMPACTION_SETTINGS,
        stream_fn: Some(&stream_fn),
        cancel: &CancellationToken::new(),
    };

    let err = summarize_with_llm(&request).await.unwrap_err();

    assert!(matches!(err, SummarizeError::ContextOverflow(_)));
}
