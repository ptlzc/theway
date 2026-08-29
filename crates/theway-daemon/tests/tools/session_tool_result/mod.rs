//! Tests for `session_tool_result` and `session_tool_result_grep`.

use super::*;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use theway_contract::session::{SessionBinding, SessionError, SessionErrorCode, SessionReader, SessionStore, StoredSessionEntry};
use theway_core::encode_session_entry;
use theway_llm_provider::{ToolResultMessage, ToolResultRole};
use tokio_util::sync::CancellationToken;

fn tool_result_message(call_id: &str, tool_name: &str, content: &str) -> ToolResultMessage {
    ToolResultMessage {
        role: ToolResultRole::ToolResult,
        tool_call_id: call_id.into(),
        tool_name: tool_name.into(),
        content: vec![UserContentBlock::text(content)],
        details: Some(json!({ "exitCode": 0 })),
        is_error: false,
        timestamp: 0,
    }
}

fn stored_tool_result(id: &str, call_id: &str, tool_name: &str, content: &str) -> StoredSessionEntry {
    let entry = SessionTreeEntry::Message {
        id: id.into(),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00Z".into(),
        message: AgentMessage::Llm(PiMessage::ToolResult(tool_result_message(
            call_id,
            tool_name,
            content,
        ))),
    };
    encode_session_entry(&entry).expect("encode fixture")
}

fn stored_tool_result_with_full(
    id: &str,
    call_id: &str,
    tool_name: &str,
    content: &str,
    full_text: &str,
) -> StoredSessionEntry {
    let mut message = tool_result_message(call_id, tool_name, content);
    message.details = Some(json!({ "exitCode": 0, "full_text": full_text }));
    let entry = SessionTreeEntry::Message {
        id: id.into(),
        parent_id: None,
        timestamp: "2026-01-01T00:00:00Z".into(),
        message: AgentMessage::Llm(PiMessage::ToolResult(message)),
    };
    encode_session_entry(&entry).expect("encode fixture")
}

#[derive(Default)]
struct FakeStore {
    entries: Mutex<Vec<StoredSessionEntry>>,
}

#[async_trait]
impl SessionReader for FakeStore {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        Ok(json!({ "id": "session-test" }))
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        Ok(self.entries.lock().unwrap().last().map(|e| e.id.clone()))
    }

    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError> {
        Ok(self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned())
    }

    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self.entries.lock().unwrap().clone())
    }

    async fn get_path_to_root(
        &self,
        _leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self.entries.lock().unwrap().clone())
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<StoredSessionEntry>, SessionError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.entry_type == entry_type)
            .cloned()
            .collect())
    }

    async fn get_label(&self, _id: &str) -> Result<Option<String>, SessionError> {
        Ok(None)
    }
}

#[async_trait]
impl SessionStore for FakeStore {
    async fn set_leaf_id(&self, _id: Option<String>) -> Result<(), SessionError> {
        Ok(())
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        Ok("entry-new".into())
    }

    async fn append_entries(&self, entries: Vec<StoredSessionEntry>) -> Result<(), SessionError> {
        self.entries.lock().unwrap().extend(entries);
        Ok(())
    }

    async fn set_binding(&self, _binding: Option<SessionBinding>) -> Result<(), SessionError> {
        Err(SessionError::new(
            SessionErrorCode::StorageFailure,
            "fake store does not support binding",
        ))
    }
}

struct FakeRepo {
    store: Arc<dyn SessionStore>,
}

#[async_trait]
impl crate::runtime_storage::SessionRepository for FakeRepo {
    async fn create(&self, _cwd: &Path) -> anyhow::Result<Arc<dyn SessionStore>> {
        Ok(self.store.clone())
    }

    async fn resume(&self, _explicit_id: Option<&str>) -> anyhow::Result<Arc<dyn SessionStore>> {
        Ok(self.store.clone())
    }

    async fn contains(&self, _id: &str) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn open(&self, _id: &str) -> anyhow::Result<Option<Arc<dyn SessionStore>>> {
        Ok(Some(self.store.clone()))
    }

    async fn list(&self) -> anyhow::Result<Vec<crate::runtime_storage::SessionRecord>> {
        Ok(Vec::new())
    }

    async fn delete(&self, _id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn fork(
        &self,
        _cwd: &Path,
        _parent: &theway_core::Session,
        _entries: Vec<StoredSessionEntry>,
    ) -> anyhow::Result<Arc<dyn SessionStore>> {
        Ok(self.store.clone())
    }

    async fn import(&self, _archive_path: &Path, _cwd: &Path) -> anyhow::Result<crate::runtime_storage::SessionImport> {
        unimplemented!("fake repo does not support import")
    }
}

fn read_tool(store: Arc<FakeStore>) -> SessionToolResultReadTool {
    let repo: Arc<dyn crate::runtime_storage::SessionRepository> =
        Arc::new(FakeRepo { store });
    SessionToolResultReadTool {
        ctx: Arc::new(SessionToolResultContext {
            repo,
            session_id: "session-test".into(),
        }),
    }
}

fn grep_tool(store: Arc<FakeStore>) -> SessionToolResultGrepTool {
    let repo: Arc<dyn crate::runtime_storage::SessionRepository> =
        Arc::new(FakeRepo { store });
    SessionToolResultGrepTool {
        ctx: Arc::new(SessionToolResultContext {
            repo,
            session_id: "session-test".into(),
        }),
    }
}

#[tokio::test]
async fn read_returns_paginated_chunk_and_has_more() {
    let store = Arc::new(FakeStore::default());
    store
        .append_entries(vec![stored_tool_result(
            "e1",
            "call_1",
            "bash",
            "line1\nline2\nline3\nline4\nline5\n",
        )])
        .await
        .unwrap();
    let tool = read_tool(store);

    let first = tool
        .execute(
            "r",
            json!({ "tool_call_id": "call_1", "offset": 1, "max_lines": 2 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let text = match &first.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("line1\nline2"), "got: {text}");
    assert!(text.contains("has_more"), "got: {text}");
    assert_eq!(first.details["total_lines"], 5);
    assert_eq!(first.details["has_more"], true);

    let last = tool
        .execute(
            "r",
            json!({ "tool_call_id": "call_1", "offset": 5, "max_lines": 2 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let last_text = match &last.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(last_text.contains("line5"), "got: {last_text}");
    assert_eq!(last.details["has_more"], false);
}

#[tokio::test]
async fn read_uses_full_text_from_details_when_content_is_truncated() {
    let store = Arc::new(FakeStore::default());
    store
        .append_entries(vec![stored_tool_result_with_full(
            "e1",
            "call_1",
            "bash",
            "truncated line\n",
            "line1\nline2\nline3\n",
        )])
        .await
        .unwrap();
    let tool = read_tool(store);

    let result = tool
        .execute(
            "r",
            json!({ "tool_call_id": "call_1", "offset": 1, "max_lines": 10 }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("line3"), "full_text should be read: {text}");
    assert_eq!(result.details["total_lines"], 3);
}

#[tokio::test]
async fn read_unknown_call_id_errors() {
    let store = Arc::new(FakeStore::default());
    let tool = read_tool(store);
    let err = tool
        .execute(
            "r",
            json!({ "tool_call_id": "missing" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("unknown id must fail");
    assert!(err.to_string().contains("not found"), "got: {err}");
}

#[test]
fn chunk_text_respects_byte_cap_and_reports_has_more() {
    let text = format!("{}\n", "a".repeat(200 * 1024)).repeat(2);
    let (chunk, kept, has_more) = chunk_text(&text, 1, 10, MAX_READ_BYTES);
    assert!(has_more);
    assert!(kept > 0);
    assert!(chunk.len() <= MAX_READ_BYTES);
}

#[tokio::test]
async fn grep_returns_line_numbers_and_matches() {
    let store = Arc::new(FakeStore::default());
    store
        .append_entries(vec![stored_tool_result(
            "e1",
            "call_1",
            "bash",
            "alpha\nbeta\nalpha gamma\n",
        )])
        .await
        .unwrap();
    let tool = grep_tool(store);

    let result = tool
        .execute(
            "g",
            json!({ "tool_call_id": "call_1", "pattern": "alpha" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("1:alpha"), "got: {text}");
    assert!(text.contains("3:alpha gamma"), "got: {text}");
    assert_eq!(result.details["matches"].as_array().unwrap().len(), 2);
    assert_eq!(result.details["truncated"], false);
}

#[tokio::test]
async fn grep_uses_full_text_from_details_when_content_is_truncated() {
    let store = Arc::new(FakeStore::default());
    store
        .append_entries(vec![stored_tool_result_with_full(
            "e1",
            "call_1",
            "bash",
            "truncated line\n",
            "alpha\nbeta\ngamma\n",
        )])
        .await
        .unwrap();
    let tool = grep_tool(store);

    let result = tool
        .execute(
            "g",
            json!({ "tool_call_id": "call_1", "pattern": "gamma" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("3:gamma"), "full_text should be grepped: {text}");
}

#[tokio::test]
async fn grep_long_line_truncates() {
    let long_line = format!("needle {}", "x".repeat(600));
    let store = Arc::new(FakeStore::default());
    store
        .append_entries(vec![stored_tool_result(
            "e1",
            "call_1",
            "bash",
            &format!("{long_line}\n"),
        )])
        .await
        .unwrap();
    let tool = grep_tool(store);

    let result = tool
        .execute(
            "g",
            json!({ "tool_call_id": "call_1", "pattern": "needle" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let text = match &result.content[0] {
        UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text"),
    };
    assert!(text.contains("[line truncated]"), "got: {text}");
    assert_eq!(result.details["truncated"], true);
    assert_eq!(result.details["matches"][0]["line_no"], 1);
}

#[tokio::test]
async fn grep_unknown_call_id_errors() {
    let store = Arc::new(FakeStore::default());
    let tool = grep_tool(store);
    let err = tool
        .execute(
            "g",
            json!({ "tool_call_id": "missing", "pattern": "x" }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("unknown id must fail");
    assert!(err.to_string().contains("not found"), "got: {err}");
}
