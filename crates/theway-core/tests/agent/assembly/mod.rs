//! Tests for `agent::assembly` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
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
