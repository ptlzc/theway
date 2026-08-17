//! Tests for `repo_utils` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use theway_core::{AgentMessage, MemorySessionStorage, Session, SessionStorage};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: 0,
        },
    ))
}

fn assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::Assistant(
        theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![theway_llm_provider::ContentBlock::text(text)],
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
    ))
}

fn storage_with_session() -> (Arc<dyn SessionStorage>, Session) {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage.clone());
    (storage, session)
}

#[test]
fn create_session_id_returns_non_empty_uuidv7_string() {
    // Arrange/Act
    let id = create_session_id();

    // Assert
    assert_eq!(id.len(), 36);
    assert_ne!(id, create_session_id());
}

#[test]
fn create_timestamp_parses_as_rfc3339() {
    // Arrange/Act
    let timestamp = create_timestamp();

    // Assert
    chrono::DateTime::parse_from_rfc3339(&timestamp).expect("timestamp must be RFC 3339");
}

#[tokio::test]
async fn to_session_wraps_the_given_storage() {
    // Arrange
    let (storage, session) = storage_with_session();

    // Act
    let wrapped = to_session(storage.clone());
    wrapped.append_message(user_message("hello")).await.unwrap();

    // Assert
    assert_eq!(session.entries().await.unwrap().len(), 1);
    assert_eq!(storage.get_entries().await.unwrap().len(), 1);
}

#[test]
fn fork_options_default_to_before_without_entry_id() {
    // Arrange/Act
    let options = ForkOptions::default();

    // Assert
    assert!(options.entry_id.is_none());
    assert!(matches!(options.position, ForkPosition::Before));
}

#[tokio::test]
async fn get_entries_to_fork_without_entry_id_returns_all_entries() {
    // Arrange
    let (storage, session) = storage_with_session();
    let _id1 = session.append_message(user_message("one")).await.unwrap();
    let _id2 = session.append_message(user_message("two")).await.unwrap();
    let _id3 = session.append_message(user_message("three")).await.unwrap();

    // Act
    let entries = get_entries_to_fork(storage.as_ref(), ForkOptions::default())
        .await
        .unwrap();

    // Assert
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn get_entries_to_fork_at_position_replays_through_target_entry() {
    // Arrange
    let (storage, session) = storage_with_session();
    let id1 = session.append_message(user_message("one")).await.unwrap();
    let id2 = session.append_message(user_message("two")).await.unwrap();
    let _id3 = session.append_message(user_message("three")).await.unwrap();

    // Act
    let entries = get_entries_to_fork(
        storage.as_ref(),
        ForkOptions {
            entry_id: Some(id2.clone()),
            position: ForkPosition::At,
        },
    )
    .await
    .unwrap();

    // Assert
    let ids: Vec<&str> = entries.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id1.as_str(), id2.as_str()]);
}

#[tokio::test]
async fn get_entries_to_fork_before_user_message_splits_before_it() {
    // Arrange
    let (storage, session) = storage_with_session();
    let id1 = session.append_message(user_message("one")).await.unwrap();
    let id2 = session.append_message(user_message("two")).await.unwrap();
    let id3 = session.append_message(user_message("three")).await.unwrap();

    // Act
    let entries = get_entries_to_fork(
        storage.as_ref(),
        ForkOptions {
            entry_id: Some(id3.clone()),
            position: ForkPosition::Before,
        },
    )
    .await
    .unwrap();

    // Assert
    let ids: Vec<&str> = entries.iter().map(|e| e.id()).collect();
    assert_eq!(ids, vec![id1.as_str(), id2.as_str()]);
}

#[tokio::test]
async fn get_entries_to_fork_before_root_user_message_returns_empty_path() {
    // Arrange
    let (storage, session) = storage_with_session();
    let id1 = session.append_message(user_message("one")).await.unwrap();

    // Act
    let entries = get_entries_to_fork(
        storage.as_ref(),
        ForkOptions {
            entry_id: Some(id1.clone()),
            position: ForkPosition::Before,
        },
    )
    .await
    .unwrap();

    // Assert
    assert!(entries.is_empty());
}

#[tokio::test]
async fn get_entries_to_fork_before_assistant_message_returns_not_found() {
    // Arrange
    let (storage, session) = storage_with_session();
    let _id1 = session.append_message(user_message("one")).await.unwrap();
    let assistant_id = session
        .append_message(assistant_message("done"))
        .await
        .unwrap();

    // Act
    let err = get_entries_to_fork(
        storage.as_ref(),
        ForkOptions {
            entry_id: Some(assistant_id.clone()),
            position: ForkPosition::Before,
        },
    )
    .await
    .unwrap_err();

    // Assert
    assert_eq!(err.code, theway_core::SessionErrorCode::NotFound);
    assert!(err.message.contains("not a user message"));
}

#[tokio::test]
async fn get_entries_to_fork_with_unknown_entry_id_returns_not_found() {
    // Arrange
    let (storage, session) = storage_with_session();
    let _id1 = session.append_message(user_message("one")).await.unwrap();

    // Act
    let err = get_entries_to_fork(
        storage.as_ref(),
        ForkOptions {
            entry_id: Some("missing".into()),
            position: ForkPosition::At,
        },
    )
    .await
    .unwrap_err();

    // Assert
    assert_eq!(err.code, theway_core::SessionErrorCode::NotFound);
    assert!(err.message.contains("missing"));
}
