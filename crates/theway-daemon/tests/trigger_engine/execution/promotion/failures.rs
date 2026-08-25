//! Additional failure-path and edge-case tests for `trigger_engine::execution::promotion`.
//!
//! Split into this submodule to keep the main promotion test module under the
//! repository's 800-line-per-file limit.

use super::*;

#[test]
fn build_template_context_uses_empty_string_for_missing_payload_summary() {
    let mut trigger = trigger_with_source(TriggerSource::Local {
        subkind: "cron".into(),
    });
    trigger.payload_summary = None;

    let ctx = build_template_context("trace-ctx", &trigger, true, &Some("sum".into()), 1);

    assert_eq!(ctx.get("trigger.payload_summary").unwrap(), "");
}

struct FailingAllStorage {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait]
impl SessionStorage for FailingAllStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }
    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: SessionErrorCode::StorageFailure,
            message: "synthetic write failure".into(),
        })
    }
    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }
    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }
    async fn get_path_to_root(
        &self,
        entry_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(entry_id).await
    }
    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }
    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }
    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn apply_promotion_render_failure_audit_append_failure_emits_persistence_error() {
    let session = Session::new(Arc::new(FailingAllStorage {
        inner: Arc::new(MemorySessionStorage::new()),
    }) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: Some("{{ trigger.authority.allowed_source_actions }}".into()),
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("(failed) append failed")
    )));
}

#[tokio::test]
async fn apply_promotion_pending_audit_append_failure_emits_persistence_error() {
    let session = Session::new(Arc::new(FailingAllStorage {
        inner: Arc::new(MemorySessionStorage::new()),
    }) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: None,
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        true,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("(pending) append failed")
    )));
    assert!(
        evs.iter()
            .any(|e| matches!(e, TriggerEvent::PromotionPending { .. }))
    );
}

#[tokio::test]
async fn apply_promotion_message_append_failure_emits_persistence_error() {
    let session = Session::new(Arc::new(FailingAllStorage {
        inner: Arc::new(MemorySessionStorage::new()),
    }) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: None,
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("promotion message append failed")
    )));
}

struct FailingTriggerPromotionStorage {
    inner: Arc<MemorySessionStorage>,
}

#[async_trait]
impl SessionStorage for FailingTriggerPromotionStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }
    async fn append_entry(&self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        if matches!(
            &entry,
            SessionTreeEntry::Custom { custom_type, .. } if custom_type == "trigger_promotion"
        ) {
            return Err(SessionError {
                code: SessionErrorCode::StorageFailure,
                message: "synthetic trigger_promotion failure".into(),
            });
        }
        self.inner.append_entry(entry).await
    }
    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }
    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }
    async fn get_path_to_root(
        &self,
        entry_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(entry_id).await
    }
    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }
    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }
    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn apply_promotion_success_audit_append_failure_emits_persistence_error() {
    let session = Session::new(Arc::new(FailingTriggerPromotionStorage {
        inner: Arc::new(MemorySessionStorage::new()),
    }) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: None,
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("(success) append failed")
    )));
    assert!(
        evs.iter()
            .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. }))
    );
}

#[test]
fn truncate_on_char_boundary_walks_back_to_multibyte_boundary() {
    let body = "é".repeat(100);
    let cap = TRUNCATION_MARKER.len() + 1;

    let (out, truncated) = truncate_on_char_boundary(body, cap);

    assert!(truncated);
    assert_eq!(out, TRUNCATION_MARKER);
}

#[tokio::test]
async fn apply_promotion_condition_skip_audits_when_condition_fails() {
    use crate::trigger_engine::execution::types::PromotionCondition;

    let session = Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryWhenResultDetailsMatch {
        template_body: None,
        condition: PromotionCondition::AnyOf {
            json_pointer: "/missing".into(),
            any_of: vec!["dyn-keep".into()],
        },
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("summary".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(
        !evs.iter()
            .any(|e| matches!(e, TriggerEvent::TriggerPromoted { .. }))
    );
    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_promotion" => Some(d.clone()),
            _ => None,
        })
        .expect("skipped promotion must audit");
    assert_eq!(audit["state"], "skipped");
    assert_eq!(audit["reason"], "result_details_missing");
}

#[tokio::test]
async fn apply_promotion_render_error_unknown_field_emits_persistence_error() {
    let session = Session::new(Arc::new(MemorySessionStorage::new()) as Arc<dyn SessionStorage>);
    let parent_agent = Arc::new(Agent::new(AgentOptions::default()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<TriggerEvent>::new()));
    let listeners = listener_vec(&events);
    let trigger = mcp_trigger();
    let promote = PromoteAction::PromoteSummaryNow {
        template_body: Some("Hello {{missing}}".into()),
    };

    apply_promotion(
        &listeners,
        &session,
        &parent_agent,
        "trace-1",
        &trigger,
        true,
        &Some("sum".into()),
        1,
        None,
        &promote,
        false,
        &serde_json::Value::Null,
    )
    .await;

    let evs = events.lock().unwrap().clone();
    assert!(evs.iter().any(|e| matches!(
        e,
        TriggerEvent::PersistenceError { context, message, }
            if context == "trigger_promotion" && message.contains("unknown template field")
    )));
    let entries = session.entries().await.unwrap();
    let audit = entries
        .iter()
        .find_map(|e| match e {
            SessionTreeEntry::Custom {
                custom_type,
                data: Some(d),
                ..
            } if custom_type == "trigger_promotion" => Some(d.clone()),
            _ => None,
        })
        .expect("failed promotion must audit");
    assert_eq!(audit["redaction_status"], "render_error");
}

#[test]
fn compute_sub_agent_outcome_success_returns_last_assistant_text() {
    let sub_agent = Agent::new(AgentOptions::default());
    sub_agent
        .state()
        .messages
        .push(assistant_message("final summary"));

    let (success, summary, message_count) = compute_sub_agent_outcome(&sub_agent, &Ok(()));

    assert!(success);
    assert_eq!(summary.as_deref(), Some("final summary"));
    assert_eq!(message_count, 1);
}

#[test]
fn last_assistant_text_ignores_non_text_blocks_and_non_assistant_messages() {
    let mut state = AgentState::default();
    state.messages.push(AgentMessage::Llm(PiMessage::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text("user".into()),
            timestamp: 0,
        },
    )));
    assert_eq!(last_assistant_text(&state), None);

    let mut state = AgentState::default();
    let msg = AgentMessage::Llm(PiMessage::Assistant(
        theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![
                theway_llm_provider::ContentBlock::Thinking(theway_llm_provider::ThinkingContent {
                    thinking: "hidden".into(),
                    thinking_signature: None,
                    redacted: false,
                }),
                theway_llm_provider::ContentBlock::text("visible"),
            ],
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
    state.messages.push(msg);
    assert_eq!(last_assistant_text(&state).as_deref(), Some("visible"));
}
