//! Goal persistence and evaluator failure paths.

use std::sync::Arc;

use super::super::*;
use crate::agent::assembly::{AgentHarness, AgentHarnessOptions};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage, SessionTreeEntry};
use crate::agent::types::SessionError;
use crate::multiagent::graph::engine::DagEngine;
use crate::multiagent::jobs::SubagentJobRegistry;
use crate::multiagent::types::AgentRunParams;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, ImageContent, Message as PiMessage, ToolResultMessage,
    ToolResultRole, UserContent, UserMessage, UserRole,
};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
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

fn harness() -> Arc<AgentHarness> {
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(MemorySessionStorage::new())),
    )))
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_with_content(content: Vec<ContentBlock>) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
        role: theway_llm_provider::AssistantRole::Assistant,
        content,
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
    }))
}

fn goal_resolver() -> crate::multiagent::types::AgentRunResolver {
    let launch = AgentRunParams {
        name: "goal-evaluator",
        description: "judge",
        system_prompt: evaluator_system_prompt(),
        max_iterations: 1,
    };
    Arc::new(move |name: &str| (name == "goal-evaluator").then_some(launch))
}

fn ctx() -> OnTurnEndContext {
    OnTurnEndContext {
        transcript: vec![user_msg("hi")],
        continuation_count: 0,
        last_user_prompt: Some("hi".into()),
    }
}

#[tokio::test]
async fn clear_without_goal_creates_cleared_state() {
    let h = harness();

    let state = clear(&h).await.unwrap();

    assert_eq!(state.condition, "");
    assert_eq!(state.status, GoalStatus::Cleared);
}

#[test]
fn latest_from_entries_skips_non_custom_and_other_custom_types() {
    let entries = vec![
        SessionTreeEntry::Message {
            id: "m1".into(),
            parent_id: None,
            timestamp: "t".into(),
            message: user_msg("hi"),
        },
        SessionTreeEntry::Custom {
            id: "c1".into(),
            parent_id: None,
            timestamp: "t".into(),
            custom_type: "other".into(),
            data: Some(serde_json::json!({"not": "goal"})),
        },
    ];

    assert!(latest_from_entries(&entries).is_none());
}

#[test]
fn agent_message_text_handles_image_only_and_empty_tool_result() {
    let image_only = assistant_with_content(vec![ContentBlock::Image(ImageContent {
        data: "base64".into(),
        mime_type: "image/png".into(),
    })]);
    assert_eq!(agent_message_text(&image_only), None);

    let empty_tool = AgentMessage::Llm(PiMessage::ToolResult(ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: "t1".into(),
        tool_name: "grep".into(),
        content: Vec::new(),
        details: None,
        is_error: false,
        timestamp: 0,
    }));
    assert_eq!(agent_message_text(&empty_tool), None);
}

#[tokio::test]
async fn ensure_goal_run_falls_back_to_existing_running_goal_run() {
    let h = harness();
    let session_id = session_id_from_harness(&h).await.unwrap();
    let engine = DagEngine::new();
    let existing = engine.plan_goal("existing", Some(session_id.clone()));

    let run_id = ensure_goal_run(&engine, &h, "existing").await.unwrap();

    assert_eq!(run_id, existing);
}

#[tokio::test]
async fn evaluate_stop_hook_returns_noop_when_goal_not_pursuing() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    pause(&h).await.unwrap();

    let decision = evaluate_stop_hook(
        h.clone(),
        Arc::new(DagEngine::new()),
        goal_resolver(),
        SubagentJobRegistry::new(),
        None,
        ctx(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(matches!(decision.action, TurnEndAction::Noop));
}

#[tokio::test]
async fn evaluate_stop_hook_pauses_when_evaluator_cancelled() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    cancel.cancel();

    let decision = evaluate_stop_hook(
        h.clone(),
        Arc::new(DagEngine::new()),
        goal_resolver(),
        SubagentJobRegistry::new(),
        None,
        ctx(),
        cancel,
    )
    .await;

    assert!(matches!(
        decision.action,
        TurnEndAction::Pause { ref reason } if reason == "goal evaluator cancelled"
    ));
}

// Storage whose append_entry always fails, to exercise the best-effort
// persistence warning path.
struct FailingAppendStorage {
    inner: MemorySessionStorage,
}

impl FailingAppendStorage {
    fn new() -> Self {
        Self {
            inner: MemorySessionStorage::new(),
        }
    }
}

#[async_trait::async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
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

    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: crate::agent::types::SessionErrorCode::StorageFailure,
            message: "append failed".into(),
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
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(leaf_id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

#[tokio::test]
async fn persist_state_best_effort_warns_when_append_fails() {
    let h = Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(FailingAppendStorage::new())),
    )));
    let state = GoalState {
        condition: "c".into(),
        status: GoalStatus::Pursuing,
        iterations: 0,
        last_reason: None,
        updated_at: chrono::Utc::now().to_rfc3339(),
    };

    persist_state_best_effort(&h, &state).await;
}
