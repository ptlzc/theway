//! Session repository construction, identifiers, timestamps, and fork helpers.

use std::sync::Arc;

use crate::types::AgentMessage;
use uuid::Uuid;

use super::super::types::{SessionError, SessionErrorCode};
use super::session::{Session, SessionStorage, SessionTreeEntry};

/// Mint a fresh UUIDv7 string (lowercase, hyphenated).
pub(crate) fn uuidv7() -> String {
    Uuid::now_v7().to_string()
}

pub fn create_session_id() -> String {
    uuidv7()
}

pub fn create_timestamp() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn to_session(storage: Arc<dyn SessionStorage>) -> Session {
    Session::new(storage)
}

/// Forking semantics: "before" (default) splits before a user message; "at" splits at a
/// specific entry id, replaying everything from the root up to and including it.
#[derive(Copy, Clone, Debug, Default)]
pub enum ForkPosition {
    #[default]
    Before,
    At,
}

#[derive(Clone, Debug, Default)]
pub struct ForkOptions {
    pub entry_id: Option<String>,
    pub position: ForkPosition,
}

pub async fn get_entries_to_fork(
    storage: &dyn SessionStorage,
    options: ForkOptions,
) -> Result<Vec<SessionTreeEntry>, SessionError> {
    let Some(entry_id) = options.entry_id.as_deref() else {
        return storage.get_entries().await;
    };
    let Some(target) = storage.get_entry(entry_id).await? else {
        return Err(SessionError {
            code: SessionErrorCode::NotFound,
            message: format!("Entry {entry_id} not found"),
        });
    };
    let effective_leaf: Option<String> = match options.position {
        ForkPosition::At => Some(target.id().to_string()),
        ForkPosition::Before => match &target {
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(m),
                parent_id,
                ..
            } if matches!(m, theway_llm_provider::Message::User(_)) => parent_id.clone(),
            _ => {
                return Err(SessionError {
                    code: SessionErrorCode::NotFound,
                    message: format!("Entry {entry_id} is not a user message"),
                });
            }
        },
    };
    storage.get_path_to_root(effective_leaf.as_deref()).await
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/session/repo_utils");
