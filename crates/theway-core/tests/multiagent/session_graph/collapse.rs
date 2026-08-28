use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{
    SESSION_GRAPH_STATE_CUSTOM_TYPE, Session, SessionGraphState, SubagentJobSnapshot,
    latest_session_graph_state,
};
use crate::multiagent::session_graph::collapse_material;
use crate::types::AgentMessage;
use theway_llm_provider::{
    AssistantMessage, ContentBlock, Message as PiMessage, StopReason, UserContent, UserMessage,
    UserRole,
};

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn assistant_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::Assistant(AssistantMessage {
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
    }))
}

#[tokio::test]
async fn append_and_read_session_graph_state_roundtrips_latest() {
    // Arrange
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let first = SessionGraphState {
        dags: Vec::new(),
        subagents: vec![SubagentJobSnapshot {
            id: "job-1".into(),
            agent: "general".into(),
            source: "dag".into(),
            run_id: Some("dag-1".into()),
            node_id: Some("a".into()),
            session_id: Some("s".into()),
            status: "running".into(),
            started_at: Some(1),
            completed_at: None,
            attempt: 1,
            total_attempts: 1,
            input_tokens: 0,
            output_tokens: 0,
            chars: 0,
            tools_called: 0,
            turn: 0,
            error: None,
            output_tail: String::new(),
            truncated: false,
            live_preview: None,
            tps: None,
            cps: None,
        }],
    };
    let second = SessionGraphState {
        subagents: first.subagents.clone(),
        ..first.clone()
    };

    // Act
    session.append_session_graph_state(&first).await.unwrap();
    session.append_session_graph_state(&second).await.unwrap();
    let read = session.session_graph_state().await.unwrap().unwrap();

    // Assert
    assert_eq!(read.subagents[0].id, "job-1");

    let entries = session.entries().await.unwrap();
    assert!(matches!(
        &entries[0],
        crate::agent::session::session::SessionTreeEntry::Custom {
            custom_type,
            ..
        } if custom_type == SESSION_GRAPH_STATE_CUSTOM_TYPE
    ));
    assert_eq!(latest_session_graph_state(&entries).unwrap().subagents[0].id, "job-1");
}

#[tokio::test]
async fn collapse_material_reads_compaction_summary_session_id_and_graph_state() {
    // Arrange
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone());
    session.append_message(user_msg("hello")).await.unwrap();
    session.append_message(assistant_msg("world")).await.unwrap();
    session
        .append_compaction("compacted summary", "first", 10, None, true)
        .await
        .unwrap();
    let state = SessionGraphState {
        dags: Vec::new(),
        subagents: Vec::new(),
    };
    session.append_session_graph_state(&state).await.unwrap();
    let expected_ref = session.session_id().await.unwrap().unwrap();

    // Act
    let (compact_text, raw_text_ref, subagent_graph) = collapse_material(&session).await;

    // Assert
    assert_eq!(compact_text, "compacted summary");
    assert_eq!(raw_text_ref, expected_ref);
    assert!(subagent_graph.dags.is_empty());
    assert!(subagent_graph.subagents.is_empty());
}

#[tokio::test]
async fn collapse_material_accepts_bare_graph_state() {
    // Arrange
    let state = SessionGraphState::default();

    // Act
    let (compact_text, raw_text_ref, subagent_graph) = collapse_material(&state).await;

    // Assert
    assert_eq!(compact_text, "");
    assert_eq!(raw_text_ref, "");
    assert_eq!(subagent_graph, state);
}

#[tokio::test]
async fn collapse_material_without_graph_state_uses_default() {
    // Arrange
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);

    // Act
    let (compact_text, raw_text_ref, subagent_graph) = collapse_material(&session).await;

    // Assert
    assert_eq!(compact_text, "");
    assert!(!raw_text_ref.is_empty());
    assert!(subagent_graph.dags.is_empty());
    assert!(subagent_graph.subagents.is_empty());
}
