//! Prompt cycle: image and template prompts, `continue_`, and on-turn-end
//! hook decisions.

use super::*;

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
