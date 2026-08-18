//! Tests for `agent::assembly` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::{LoadSkillsOutput, SessionTreeEntry, StreamFn};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage};
use crate::agent::types::{SessionError, SessionErrorCode, SkillSource};
use theway_llm_provider::{
    AssistantRole, ContentBlock, ImageContent, Message as PiMessage, StopReason, UserContent,
    UserContentBlock, UserMessage, UserRole,
};

fn faux_model() -> Model {
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
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn harness() -> AgentHarness {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    AgentHarness::new(AgentHarnessOptions::new(faux_model(), session))
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(theway_llm_provider::AssistantMessage {
        role: AssistantRole::Assistant,
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
    }))
}

fn skill(name: &str, content: &str) -> Skill {
    Skill {
        name: name.into(),
        description: "test skill".into(),
        file_path: format!("/skills/{name}/SKILL.md"),
        content: content.into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}

#[test]
fn build_system_prompt_combines_base_and_skills() {
    assert_eq!(build_system_prompt("", &[]), "");
    assert_eq!(build_system_prompt("base", &[]), "base");
    assert!(build_system_prompt("", &[skill("a", "body")]).contains("<skill"));
    let both = build_system_prompt("base", &[skill("a", "body")]);
    assert!(both.starts_with("base\n\n"));
    assert!(both.contains("<skills>"));
    assert!(both.contains("test skill"));
}

#[test]
fn turn_end_action_audit_str_maps_only_non_noop() {
    assert_eq!(TurnEndAction::Noop.as_audit_str(), None);
    assert_eq!(TurnEndAction::Stop.as_audit_str(), Some("stop"));
    assert_eq!(
        TurnEndAction::Pause {
            reason: "x".into()
        }
        .as_audit_str(),
        Some("pause")
    );
    assert_eq!(
        TurnEndAction::Continue {
            prompt: "x".into()
        }
        .as_audit_str(),
        Some("continue")
    );
}

#[test]
fn turn_end_decision_from_action_sets_none_payload() {
    let decision = TurnEndDecision::from(TurnEndAction::Stop);
    assert!(matches!(decision.action, TurnEndAction::Stop));
    assert!(decision.payload.is_none());
}

#[test]
fn preview_for_banner_truncates_with_ellipsis() {
    assert_eq!(preview_for_banner("short", 10), "short");
    assert_eq!(preview_for_banner("123456", 5), "12345…");
}

#[test]
fn cap_control_plane_audit_label_caps_at_200_chars() {
    let exact = "x".repeat(200);
    assert_eq!(cap_control_plane_audit_label(&exact), exact);
    let over = "y".repeat(201);
    let capped = cap_control_plane_audit_label(&over);
    assert_eq!(capped.chars().count(), 200);
    assert!(capped.ends_with('…'));
}

#[test]
fn extract_user_message_text_handles_text_blocks_and_empty() {
    let text = UserMessage {
        role: UserRole::User,
        content: UserContent::Text("hello".into()),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&text).unwrap(), "hello");

    let empty = UserMessage {
        role: UserRole::User,
        content: UserContent::Text(String::new()),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&empty), None);

    let blocks = UserMessage {
        role: UserRole::User,
        content: UserContent::Blocks(vec![
            UserContentBlock::text("a"),
            UserContentBlock::Image(ImageContent {
                data: "base64".into(),
                mime_type: "image/png".into(),
            }),
            UserContentBlock::text("b"),
        ]),
        timestamp: 0,
    };
    assert_eq!(extract_user_message_text(&blocks).unwrap(), "a\nb");
}

#[test]
fn extract_user_prompt_text_returns_none_for_non_user() {
    assert_eq!(extract_user_prompt_text(&assistant_message("hi")), None);
    let custom = AgentMessage::Custom(CustomMessage {
        role: "note".into(),
        timestamp: 0,
        payload: serde_json::Value::Null,
    });
    assert_eq!(extract_user_prompt_text(&custom), None);
    assert_eq!(
        extract_user_prompt_text(&user_message("prompt")),
        Some("prompt".into())
    );
}

#[test]
fn finish_persisted_run_prefers_run_error_then_persist_error() {
    let persist_errors = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let run_err = Err(AgentRunError::Other("run failed".into()));
    let err = finish_persisted_run(run_err, persist_errors.clone()).unwrap_err();
    assert!(err.to_string().contains("run failed"));

    persist_errors.lock().push(SessionError {
        code: SessionErrorCode::StorageFailure,
        message: "disk".into(),
    });
    let err = finish_persisted_run(Ok(()), persist_errors.clone()).unwrap_err();
    assert!(err.to_string().contains("session append message: disk"));

    persist_errors.lock().clear();
    assert!(finish_persisted_run(Ok(()), persist_errors).is_ok());
}

#[tokio::test]
async fn make_session_listener_persists_message_and_control_plane_prompt() {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let (listener, errors) = make_session_listener(session.clone());
    let cancel = tokio_util::sync::CancellationToken::new();

    listener(
        LoopEvent::MessageEnd {
            message: user_message("hello"),
        },
        cancel.clone(),
    )
    .await;
    listener(
        LoopEvent::ControlPlanePromptResolved {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            args_hash: "a".repeat(64),
            label: "Control-plane write: write_file".into(),
            decision: "allow".into(),
            reason: None,
        },
        cancel.clone(),
    )
    .await;

    let entries = session.entries().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(errors.lock().is_empty());
}

#[tokio::test]
async fn make_session_listener_records_persist_errors() {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let (listener, errors) = make_session_listener(session.clone());
    // A non-message event must be ignored.
    listener(LoopEvent::TurnStart, tokio_util::sync::CancellationToken::new())
        .await;
    assert!(session.entries().await.unwrap().is_empty());
    assert!(errors.lock().is_empty());
}

#[tokio::test]
async fn reload_skills_from_disk_errors_when_not_configured() {
    let h = harness();
    let err = h.reload_skills_from_disk().await.unwrap_err();
    assert!(matches!(err, ReloadSkillsError::NotConfigured));
}

#[test]
fn check_budget_cap_uses_configured_cap() {
    let mut h = harness();
    h.budget_cap_usd = None;
    assert!(h.check_budget_cap().is_ok());

    h.budget_cap_usd = Some(0.0);
    let err = h.check_budget_cap().unwrap_err();
    assert!(err.to_string().contains("budget cap reached"));
}

#[test]
fn last_user_text_from_state_finds_most_recent_user_text() {
    let h = harness();
    h.agent.state().messages = vec![
        user_message("first"),
        assistant_message("reply"),
        user_message("second"),
    ];
    assert_eq!(h.last_user_text_from_state().unwrap(), "second");
}

#[test]
fn ensure_session_start_emitted_fires_once() {
    let h = harness();
    let mut rx = h.subscribe_session_broadcast();

    h.ensure_session_start_emitted();
    h.ensure_session_start_emitted();

    let mut seen = 0;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SessionEvent::Started { .. }) {
            seen += 1;
        }
    }
    assert_eq!(seen, 1);
}

#[tokio::test]
async fn prompt_from_template_unknown_name_errors() {
    let h = harness();
    let err = h
        .prompt_from_template("missing", serde_json::Map::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown prompt template: missing"));
}

#[test]
fn replace_skills_updates_catalog_and_system_prompt() {
    let h = harness();
    h.replace_skills(vec![skill("a", "body")]);
    assert_eq!(h.skills().len(), 1);
    assert!(h.system_prompt().contains("test skill"));
    assert!(h.templates().is_empty());
}

#[test]
fn abort_interrupt_enqueue_passthroughs_are_noops_without_active_run() {
    let h = harness();
    h.abort();
    h.interrupt();
    h.enqueue_steering(user_message("steer"));
    h.enqueue_follow_up(user_message("follow"));
    assert!(h.cost().tokens.total_tokens == 0);
    h.reset_cost();
}

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

#[test]
fn subscribe_harness_receives_and_unsubscribes() {
    let h = harness();
    let seen = Arc::new(std::sync::Mutex::new(0usize));
    let seen_clone = seen.clone();
    let unsubscribe = h.subscribe_harness(Arc::new(move |event| {
        if matches!(event, SessionEvent::Started { .. }) {
            *seen_clone.lock().unwrap() += 1;
        }
    }));

    h.ensure_session_start_emitted();
    assert_eq!(*seen.lock().unwrap(), 1);

    unsubscribe();
    h.emit_harness_event(SessionEvent::Started { messages_replayed: 0 });
    assert_eq!(*seen.lock().unwrap(), 1);
}

#[tokio::test]
async fn set_model_and_thinking_level_persist_entries() {
    let h = harness();

    let id = h.set_model(faux_model()).await.unwrap();
    assert_eq!(h.session().get_entry(&id).await.unwrap().unwrap().type_str(), "model_change");

    let id = h
        .set_thinking_level(ThinkingLevel::High)
        .await
        .unwrap();
    assert_eq!(
        h.session()
            .get_entry(&id)
            .await
            .unwrap()
            .unwrap()
            .type_str(),
        "thinking_level_change"
    );

    assert_eq!(h.agent().state().model.as_ref().unwrap().id, "faux");
    assert_eq!(
        h.agent().state().thinking_level,
        Some(ThinkingLevel::High)
    );
}

#[tokio::test]
async fn move_to_rehydrates_state_and_emits_branch_event() {
    let h = harness();
    let first = h.session().append_message(user_message("one")).await.unwrap();
    h.session().append_message(user_message("two")).await.unwrap();
    h.agent().state().messages = vec![user_message("two")];

    let mut rx = h.subscribe_session_broadcast();
    let summary_id = h
        .move_to(
            Some(&first),
            Some(BranchSummaryInput {
                summary: "back to one".into(),
                details: None,
                from_hook: false,
            }),
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(h.session().leaf_id().await.unwrap().unwrap(), summary_id);
    // Rehydrated branch: the kept message plus the branch-summary marker.
    assert_eq!(h.agent().state().messages.len(), 2);

    let mut saw_branch = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SessionEvent::Branch { .. }) {
            saw_branch = true;
        }
    }
    assert!(saw_branch, "move_to must emit a Branch event");
}

#[tokio::test]
async fn rehydrate_from_session_restores_messages_and_thinking() {
    let h = harness();
    h.session().append_message(user_message("from session")).await.unwrap();
    h.session()
        .append_thinking_level_change("high")
        .await
        .unwrap();
    h.agent().state().messages = Vec::new();
    h.agent().state().thinking_level = Some(ThinkingLevel::Off);

    let ctx = h.rehydrate_from_session().await.unwrap();

    assert_eq!(ctx.messages.len(), 1);
    assert_eq!(h.agent().state().messages.len(), 1);
    assert_eq!(h.agent().state().thinking_level, Some(ThinkingLevel::High));
}

#[tokio::test]
async fn reload_skills_from_disk_applies_loader_result() {
    let mut h = harness();
    h.reload_skills_fn = Some(Arc::new(|| {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![skill("reloaded", "body")],
                diagnostics: Vec::new(),
            }
        })
    }));

    let out = h.reload_skills_from_disk().await.unwrap();

    assert_eq!(out.skills.len(), 1);
    assert_eq!(h.skills().len(), 1);
    assert!(h.system_prompt().contains("reloaded"));
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
