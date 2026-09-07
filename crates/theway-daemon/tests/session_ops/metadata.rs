use super::*;
use super::super::metadata::{
    append_session_metadata, compact_context_entries, read_session_metadata, transcript_material,
};

#[tokio::test]
async fn read_session_metadata_skips_non_matching_and_non_object_entries() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let store: Arc<dyn theway_contract::session::SessionStore> = Arc::new(session);
    let facade = theway_core::Session::from_store(store.clone());

    facade
        .append_custom("other", Some(serde_json::json!({ "ignored": true })))
        .await
        .unwrap();
    append_session_metadata(
        store.as_ref(),
        &[("tenant", "acme"), ("env", "prod")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
    .await
    .unwrap();

    let meta = read_session_metadata(store.as_ref()).await.unwrap();
    assert_eq!(meta.get("tenant").map(String::as_str), Some("acme"));
    assert_eq!(meta.get("env").map(String::as_str), Some("prod"));
    assert!(!meta.contains_key("ignored"));
}

#[tokio::test]
async fn append_session_metadata_round_trips_through_read() {
    let dir = tempdir().unwrap();
    let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
    let session = repo.create("/cwd").await.unwrap();
    let store: Arc<dyn theway_contract::session::SessionStore> = Arc::new(session);

    let mut first = HashMap::new();
    first.insert("a".to_string(), "1".to_string());
    append_session_metadata(store.as_ref(), &first).await.unwrap();

    let mut second = HashMap::new();
    second.insert("b".to_string(), "2".to_string());
    append_session_metadata(store.as_ref(), &second).await.unwrap();

    let meta = read_session_metadata(store.as_ref()).await.unwrap();
    assert_eq!(meta.get("a").map(String::as_str), Some("1"));
    assert_eq!(meta.get("b").map(String::as_str), Some("2"));
}

fn memory_session() -> theway_core::Session {
    let storage = Arc::new(theway_core::MemorySessionStorage::new());
    theway_core::Session::new(storage as Arc<dyn theway_core::SessionStorage>)
}

#[tokio::test]
async fn transcript_material_collects_text_and_skips_tool_results() {
    let session = memory_session();

    session
        .append_message(theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text("plain user".into()),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();
    session
        .append_message(theway_core::AgentMessage::Llm(theway_llm_provider::Message::Assistant(
            theway_llm_provider::AssistantMessage {
                role: theway_llm_provider::AssistantRole::Assistant,
                content: vec![
                    theway_llm_provider::ContentBlock::text("assistant text"),
                    theway_llm_provider::ContentBlock::Image(theway_llm_provider::ImageContent {
                        data: "x".into(),
                        mime_type: "image/png".into(),
                    }),
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
        )))
        .await
        .unwrap();
    session
        .append_message(theway_core::AgentMessage::Llm(theway_llm_provider::Message::ToolResult(
            theway_llm_provider::ToolResultMessage {
                role: theway_llm_provider::ToolResultRole::ToolResult,
                tool_call_id: "t1".into(),
                tool_name: "read".into(),
                content: vec![theway_llm_provider::UserContentBlock::text("tool output")],
                details: None,
                is_error: false,
                timestamp: 0,
            },
        )))
        .await
        .unwrap();

    let material = transcript_material(&session).await.unwrap();
    assert!(material.contains("plain user"));
    assert!(material.contains("assistant text"));
    assert!(!material.contains("tool output"));
    assert!(!material.is_empty());
}

#[tokio::test]
async fn compact_context_entries_builds_two_entries_with_graph_state() {
    let state = theway_core::multiagent::session_graph::SessionGraphState::default();
    let entries = compact_context_entries("source", "summary", "raw", &state, Some("parent-1"))
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].payload["customType"], "compact_context");
    assert_eq!(entries[0].payload["parentId"], "parent-1");
    assert_eq!(entries[1].payload["customType"], "session_graph_state");
    let graph = &entries[1].payload["data"];
    assert_eq!(graph["dags"], serde_json::json!([]));
    assert_eq!(graph["subagents"], serde_json::json!([]));
}

#[tokio::test]
async fn transcript_material_handles_user_blocks_and_blank_text() {
    let session = memory_session();

    session
        .append_message(theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(
            theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Blocks(vec![
                    theway_llm_provider::UserContentBlock::text("block one"),
                    theway_llm_provider::UserContentBlock::Image(
                        theway_llm_provider::ImageContent {
                            data: "i".into(),
                            mime_type: "image/png".into(),
                        },
                    ),
                    theway_llm_provider::UserContentBlock::text(""),
                ]),
                timestamp: 0,
            },
        )))
        .await
        .unwrap();

    let material = transcript_material(&session).await.unwrap();
    assert!(material.contains("block one"));
}
