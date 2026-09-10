//! Prompt cycle: image and template prompts, structured user-input records,
//! `continue_`, and on-turn-end hook decisions.

use super::*;
use theway_contract::user_input::{
    InputFilePart, InputImagePart, InputInjectedPart, InputPart, InputSource, UserInput,
    digest_bytes,
};

// ──────────────────────────────────────────────────────────────────────────────────────────
// Harness lifecycle / prompt-cycle coverage
// ──────────────────────────────────────────────────────────────────────────────────────────

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = theway_llm_provider::AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![ContentBlock::text(text)],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: theway_llm_provider::Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 0,
            };
            sender.push(theway_llm_provider::AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(theway_llm_provider::AssistantMessageEvent::Done {
                reason: theway_llm_provider::DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn harness_with_stream(stream: StreamFn) -> AgentHarness {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stream);
    AgentHarness::new(opts)
}

#[tokio::test]
async fn prompt_with_images_and_prompt_from_template_run() {
    let h = harness_with_stream(faux_stream("ok"));
    h.templates.lock().push(PromptTemplate {
        name: "greet".into(),
        description: None,
        content: "hello {{who}}".into(),
        file_path: "/t".into(),
    });
    let mut vars = serde_json::Map::new();
    vars.insert("who".into(), serde_json::json!("world"));

    h.prompt_from_template("greet", vars).await.unwrap();
    h.prompt_with_images(
        "look",
        vec![ImageContent {
            data: "base64".into(),
            mime_type: "image/png".into(),
        }],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn continue_after_assistant_only_transcript_runs() {
    let h = harness_with_stream(faux_stream("ok"));
    h.agent().state().messages = vec![assistant_message("previous")];

    h.continue_().await.unwrap();

    assert!(h.agent().state().messages.len() >= 2);
}

#[tokio::test]
async fn on_turn_end_hook_stop_pause_and_noop_paths() {
    for action in ["stop", "pause", "noop"] {
        let mut h = harness_with_stream(faux_stream("ok"));
        let hook: OnTurnEndHook = Arc::new(move |ctx, _cancel| {
            let action = action.to_string();
            Box::pin(async move {
                assert_eq!(ctx.continuation_count, 0);
                match action.as_str() {
                    "stop" => TurnEndDecision::from(TurnEndAction::Stop),
                    "pause" => TurnEndDecision::from(TurnEndAction::Pause {
                        reason: "paused".into(),
                    }),
                    _ => TurnEndDecision::from(TurnEndAction::Noop),
                }
            })
        });
        h.on_turn_end = Some(hook);

        h.prompt("hello").await.unwrap();

        let entries = h.session().entries().await.unwrap();
        let decisions: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                SessionTreeEntry::Custom {
                    custom_type, data, ..
                } if custom_type == "turn_end_decision" => data
                    .as_ref()
                    .and_then(|d| d.get("decision"))
                    .and_then(|d| d.as_str()),
                _ => None,
            })
            .collect();
        if action == "noop" {
            assert!(decisions.is_empty(), "Noop must not write an audit entry");
        } else {
            assert_eq!(decisions, vec![action]);
        }
    }
}

#[tokio::test]
async fn on_turn_end_hook_continue_then_stop_writes_both_audit_entries() {
    let mut h = harness_with_stream(faux_stream("ok"));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let calls = calls_clone.clone();
        Box::pin(async move {
            let nth = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if nth == 0 {
                TurnEndDecision::from(TurnEndAction::Continue {
                    prompt: "keep going".into(),
                })
            } else {
                TurnEndDecision::from(TurnEndAction::Stop)
            }
        })
    });
    h.on_turn_end = Some(hook);

    h.prompt("hello").await.unwrap();

    let entries = h.session().entries().await.unwrap();
    let decisions: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "turn_end_decision" => data
                .as_ref()
                .and_then(|d| d.get("decision"))
                .and_then(|d| d.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(decisions, vec!["continue", "stop"]);
}

#[tokio::test]
async fn on_turn_end_hook_continue_respects_cap_zero() {
    let mut h = harness_with_stream(faux_stream("ok"));
    h.turn_continuation_cap = 0;
    let hook_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_calls_clone = hook_calls.clone();
    let hook: OnTurnEndHook = Arc::new(move |_ctx, _cancel| {
        let calls = hook_calls_clone.clone();
        Box::pin(async move {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            TurnEndDecision::from(TurnEndAction::Continue {
                prompt: "should not run".into(),
            })
        })
    });
    h.on_turn_end = Some(hook);

    h.prompt("hello").await.unwrap();

    // The cap is checked before the hook runs on the second iteration.
    assert_eq!(hook_calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let entries = h.session().entries().await.unwrap();
    let decisions: Vec<&str> = entries
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type, data, ..
            } if custom_type == "turn_end_decision" => data
                .as_ref()
                .and_then(|d| d.get("decision"))
                .and_then(|d| d.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(decisions, vec!["budget_limited"]);
}

#[tokio::test]
async fn prompt_with_images_with_empty_text_still_sends_images() {
    let h = harness_with_stream(faux_stream("ok"));
    h.prompt_with_images(
        "",
        vec![ImageContent {
            data: "base64".into(),
            mime_type: "image/png".into(),
        }],
    )
    .await
    .unwrap();

    let messages = h.agent.state().messages.clone();
    let user = messages.iter().find_map(|m| match m {
        AgentMessage::Llm(PiMessage::User(user)) => Some(user),
        _ => None,
    });
    let user = user.unwrap();
    match &user.content {
        UserContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 1);
            assert!(matches!(blocks[0], UserContentBlock::Image(_)));
        }
        other => panic!("expected image block, got {other:?}"),
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Structured user input — `prompt_with_input` / `record_user_input_prompt` archive the
// canonical `user_input` record one entry ahead of the user message it describes.
// ──────────────────────────────────────────────────────────────────────────────────────────

fn sample_user_input() -> UserInput {
    UserInput {
        text: "summarize @src/lib.rs".into(),
        parts: vec![
            InputPart::File(InputFilePart {
                path: "src/lib.rs".into(),
                name: "lib.rs".into(),
                digest: digest_bytes(b"lib.rs"),
                bytes: 6,
                media_type: "text/x-rust".into(),
                truncated: false,
            }),
            InputPart::Image(InputImagePart {
                name: Some("shot.png".into()),
                digest: digest_bytes(b"png"),
                bytes: 3,
                media_type: "image/png".into(),
            }),
            InputPart::Injected(InputInjectedPart {
                source: "skill".into(),
                name: Some("review".into()),
                text: "review the diff".into(),
            }),
        ],
        source: InputSource::User,
        source_ref: None,
    }
}

fn sample_image() -> ImageContent {
    ImageContent {
        data: "base64".into(),
        mime_type: "image/png".into(),
    }
}

fn is_user_message(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::Llm(PiMessage::User(_)))
}

fn is_user_input_record(message: &AgentMessage) -> bool {
    match message {
        AgentMessage::Custom(custom) => custom.role == UserInput::CUSTOM_ROLE,
        _ => false,
    }
}

fn logged_messages(entries: &[SessionTreeEntry]) -> Vec<AgentMessage> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SessionTreeEntry::Message { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn has_user_input_record(messages: &[AgentMessage]) -> bool {
    messages.iter().any(is_user_input_record)
}

fn assert_user_input_record(message: &AgentMessage, expected: &UserInput) {
    let custom = match message {
        AgentMessage::Custom(custom) => custom,
        other => panic!("expected a user_input custom message, got {other:?}"),
    };
    assert_eq!(custom.role, UserInput::CUSTOM_ROLE);
    let restored: UserInput = serde_json::from_value(custom.payload.clone())
        .expect("user_input payload must deserialize");
    assert_eq!(&restored, expected);
}

fn capturing_stream(captured: Arc<std::sync::Mutex<Vec<Vec<PiMessage>>>>) -> StreamFn {
    Arc::new(move |model, context, options| {
        captured.lock().unwrap().push(context.messages.clone());
        let stream_fn = faux_stream("ok");
        stream_fn(model, context, options)
    })
}

fn captured_messages(captured: &std::sync::Mutex<Vec<Vec<PiMessage>>>) -> Vec<PiMessage> {
    let mut calls = captured.lock().unwrap();
    match calls.len() {
        1 => calls.remove(0),
        other => panic!("expected one provider call, got {other}"),
    }
}

fn provider_user_text(message: &PiMessage) -> String {
    match message {
        PiMessage::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            other => panic!("expected text content, got {other:?}"),
        },
        other => panic!("expected a user message, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_with_input_appends_record_before_user_message() {
    let h = harness_with_stream(faux_stream("ok"));
    let input = sample_user_input();

    h.prompt_with_input("summarize @src/lib.rs", Vec::new(), Some(input.clone()))
        .await
        .unwrap();

    let entries = h.session().entries().await.unwrap();
    let logged = logged_messages(&entries);
    assert_eq!(logged.len(), 3);
    assert_user_input_record(&logged[0], &input);
    assert!(is_user_message(&logged[1]));

    let state = h.agent().state();
    assert_eq!(state.messages.len(), 3);
    assert_user_input_record(&state.messages[0], &input);
    assert!(is_user_message(&state.messages[1]));
}

#[tokio::test]
async fn prompt_with_input_with_images_appends_record_and_image_blocks() {
    let h = harness_with_stream(faux_stream("ok"));
    let input = sample_user_input();
    h.prompt_with_input("look", vec![sample_image()], Some(input.clone()))
        .await
        .unwrap();

    let state = h.agent().state();
    assert_user_input_record(&state.messages[0], &input);
    let user = match &state.messages[1] {
        AgentMessage::Llm(PiMessage::User(user)) => user,
        other => panic!("expected the user message, got {other:?}"),
    };
    match &user.content {
        UserContent::Blocks(blocks) => {
            assert_eq!(blocks.len(), 2);
            assert!(matches!(blocks[0], UserContentBlock::Text(_)));
            assert!(matches!(blocks[1], UserContentBlock::Image(_)));
        }
        other => panic!("expected content blocks, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_paths_without_input_append_no_record() {
    let imgs = vec![sample_image()];

    let h = harness_with_stream(faux_stream("ok"));
    h.prompt("hello").await.unwrap();
    let entries = h.session().entries().await.unwrap();
    assert!(!has_user_input_record(&logged_messages(&entries)));
    assert!(!has_user_input_record(&h.agent().state().messages));

    let h = harness_with_stream(faux_stream("ok"));
    h.prompt_with_images("look", imgs).await.unwrap();
    let entries = h.session().entries().await.unwrap();
    let logged = logged_messages(&entries);
    assert!(!has_user_input_record(&logged));
    assert!(is_user_message(&logged[0]));

    let h = harness_with_stream(faux_stream("ok"));
    h.prompt_with_input("hi", vec![], None).await.unwrap();
    let entries = h.session().entries().await.unwrap();
    assert!(!has_user_input_record(&logged_messages(&entries)));
}

#[tokio::test]
async fn record_user_prompt_without_input_appends_no_record() {
    let h = harness_with_stream(faux_stream("ok"));
    h.record_user_prompt("queued", Vec::new()).await.unwrap();
    let entries = h.session().entries().await.unwrap();
    assert!(!has_user_input_record(&logged_messages(&entries)));
    assert!(!has_user_input_record(&h.agent().state().messages));
}

#[tokio::test]
async fn user_input_record_round_trips_through_persisted_json() {
    let h = harness_with_stream(faux_stream("ok"));
    let input = sample_user_input();
    h.prompt_with_input("summarize @src/lib.rs", Vec::new(), Some(input.clone()))
        .await
        .unwrap();

    let entries = h.session().entries().await.unwrap();
    let encoded = serde_json::to_value(&entries[0]).unwrap();
    let decoded: SessionTreeEntry = serde_json::from_value(encoded).unwrap();
    let custom = match decoded {
        SessionTreeEntry::Message { message: AgentMessage::Custom(custom), .. } => custom,
        other => panic!("first session entry must be the user_input record, got {other:?}"),
    };
    assert_eq!(custom.role, UserInput::CUSTOM_ROLE);
    let restored: UserInput = serde_json::from_value(custom.payload).unwrap();
    assert_eq!(restored, input);
}

#[tokio::test]
async fn queued_steering_input_reaches_the_log_before_the_steering_message() {
    let h = harness_with_stream(faux_stream("ok"));
    let input = sample_user_input();
    h.enqueue_steering_input(&input).unwrap();
    h.enqueue_steering(user_message("steer"));

    h.prompt("hello").await.unwrap();

    let entries = h.session().entries().await.unwrap();
    let logged = logged_messages(&entries);
    let record_index = logged
        .iter()
        .position(is_user_input_record)
        .expect("steering record must reach the session log");
    assert_user_input_record(&logged[record_index], &input);
    let queued = match &logged[record_index + 1] {
        AgentMessage::Llm(message) => message,
        other => panic!("expected the steering message, got {other:?}"),
    };
    assert_eq!(provider_user_text(queued), "steer");
}

#[tokio::test]
async fn user_input_record_is_filtered_from_provider_messages() {
    let plain_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let plain_stream = capturing_stream(plain_seen.clone());
    let plain = harness_with_stream(plain_stream);
    plain.prompt("hello").await.unwrap();

    let recorded_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded_stream = capturing_stream(recorded_seen.clone());
    let recorded = harness_with_stream(recorded_stream);
    let input = sample_user_input();
    recorded
        .prompt_with_input("hello", Vec::new(), Some(input.clone()))
        .await
        .unwrap();

    let plain_messages = captured_messages(&plain_seen);
    let recorded_messages = captured_messages(&recorded_seen);
    assert_eq!(recorded_messages.len(), plain_messages.len());
    let plain_text = provider_user_text(&plain_messages[0]);
    let recorded_text = provider_user_text(&recorded_messages[0]);
    assert_eq!(recorded_text, plain_text);

    // The dropped record was in the transcript that fed the provider request.
    let state = recorded.agent().state();
    assert_user_input_record(&state.messages[0], &input);
}
