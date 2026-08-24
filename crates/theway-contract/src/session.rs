//! Engine-independent session persistence contracts.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::extension::ExtensionDurableEntry;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorCode {
    NotFound,
    AlreadyExists,
    Corrupted,
    StorageFailure,
    Aborted,
    Unknown,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionError {
    pub code: SessionErrorCode,
    pub message: String,
}

impl SessionError {
    pub fn new(code: SessionErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn corrupted(message: impl Into<String>) -> Self {
        Self::new(SessionErrorCode::Corrupted, message)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// Persisted session runtime values; credentials must not be added here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRuntimeContext {
    #[serde(rename = "workDir")]
    pub work_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "baseUrl")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
}

/// Persisted client identity and its non-secret runtime context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBinding {
    #[serde(rename = "clientKey")]
    pub client_key: String,
    pub runtime: SessionRuntimeContext,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JsonlSessionMetadata {
    #[serde(flatten)]
    pub base: SessionMetadata,
    pub cwd: String,
    pub path: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "parentSessionPath"
    )]
    pub parent_session_path: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "importedFrom"
    )]
    pub imported_from: Option<SessionImportOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<SessionBinding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionImportOrigin {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub cwd: String,
    #[serde(rename = "exportedAt")]
    pub exported_at: String,
    #[serde(rename = "thewayVersion")]
    pub theway_version: String,
}

/// One persisted session entry. `payload` is the canonical full tagged JSON
/// object; the remaining fields are validated indexes used by backends.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredSessionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub entry_type: String,
    pub timestamp: String,
    pub payload: Value,
}

impl StoredSessionEntry {
    pub fn from_payload(payload: Value) -> Result<Self, SessionError> {
        let object = payload
            .as_object()
            .ok_or_else(|| SessionError::corrupted("session entry must be a JSON object"))?;
        let string = |field: &str| -> Result<String, SessionError> {
            object
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    SessionError::corrupted(format!("session entry has invalid {field}"))
                })
        };
        let parent_id = match object.get("parentId") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => {
                return Err(SessionError::corrupted(
                    "session entry has invalid parentId",
                ));
            }
        };
        let entry = Self {
            id: string("id")?,
            parent_id,
            entry_type: string("type")?,
            timestamp: string("timestamp")?,
            payload,
        };
        entry.validate_shape()?;
        Ok(entry)
    }

    pub fn leaf(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        target_id: Option<String>,
    ) -> Result<Self, SessionError> {
        Self::from_payload(serde_json::json!({
            "type": "leaf",
            "id": id,
            "parentId": parent_id,
            "timestamp": timestamp,
            "targetId": target_id,
        }))
    }

    pub fn session_info(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        name: String,
    ) -> Result<Self, SessionError> {
        Self::from_payload(serde_json::json!({
            "type": "session_info",
            "id": id,
            "parentId": parent_id,
            "timestamp": timestamp,
            "name": name,
        }))
    }

    pub fn extension(
        id: String,
        parent_id: Option<String>,
        timestamp: String,
        extension: ExtensionDurableEntry,
    ) -> Result<Self, SessionError> {
        extension
            .validate()
            .map_err(|error| SessionError::corrupted(error.to_string()))?;
        Self::from_payload(serde_json::json!({
            "type": "extension",
            "id": id,
            "parentId": parent_id,
            "timestamp": timestamp,
            "extension": extension,
        }))
    }

    pub fn leaf_target_id(&self) -> Option<Option<&str>> {
        (self.entry_type == "leaf").then(|| self.payload.get("targetId").and_then(Value::as_str))
    }

    pub fn label_update(&self) -> Option<(&str, Option<&str>)> {
        if self.entry_type != "label" {
            return None;
        }
        let target = self.payload.get("targetId")?.as_str()?;
        let label = self.payload.get("label").and_then(Value::as_str);
        Some((target, label))
    }

    pub fn extension_payload(&self) -> Result<Option<ExtensionDurableEntry>, SessionError> {
        if self.entry_type != "extension" {
            return Ok(None);
        }
        let extension =
            self.payload.get("extension").cloned().ok_or_else(|| {
                SessionError::corrupted("extension session entry has no envelope")
            })?;
        let extension: ExtensionDurableEntry = serde_json::from_value(extension)
            .map_err(|error| SessionError::corrupted(error.to_string()))?;
        extension
            .validate()
            .map_err(|error| SessionError::corrupted(error.to_string()))?;
        Ok(Some(extension))
    }

    fn validate_shape(&self) -> Result<(), SessionError> {
        let object = self.payload.as_object().expect("validated object");
        let require_string = |field: &str| {
            object
                .get(field)
                .and_then(Value::as_str)
                .map(|_| ())
                .ok_or_else(|| {
                    SessionError::corrupted(format!(
                        "{} session entry has invalid {field}",
                        self.entry_type
                    ))
                })
        };
        match self.entry_type.as_str() {
            "message" => {
                if !object.get("message").is_some_and(Value::is_object) {
                    return Err(SessionError::corrupted(
                        "message session entry has invalid message",
                    ));
                }
            }
            "thinking_level_change" => require_string("thinkingLevel")?,
            "model_change" => {
                require_string("provider")?;
                require_string("modelId")?;
            }
            "compaction" => {
                require_string("summary")?;
                require_string("firstKeptEntryId")?;
                if !object.get("tokensBefore").is_some_and(Value::is_u64) {
                    return Err(SessionError::corrupted(
                        "compaction session entry has invalid tokensBefore",
                    ));
                }
            }
            "branch_summary" => {
                require_string("fromId")?;
                require_string("summary")?;
            }
            "extension" => {
                self.extension_payload()?;
            }
            "custom" => require_string("customType")?,
            "custom_message" => {
                require_string("customType")?;
                if !object.contains_key("content")
                    || !object.get("display").is_some_and(Value::is_boolean)
                {
                    return Err(SessionError::corrupted(
                        "custom_message session entry has invalid content or display",
                    ));
                }
            }
            "label" => require_string("targetId")?,
            "session_info" => {
                if object.get("name").is_some_and(|value| !value.is_string()) {
                    return Err(SessionError::corrupted(
                        "session_info session entry has invalid name",
                    ));
                }
            }
            "leaf" => {
                if object
                    .get("targetId")
                    .is_some_and(|value| !value.is_null() && !value.is_string())
                {
                    return Err(SessionError::corrupted(
                        "leaf session entry has invalid targetId",
                    ));
                }
            }
            other => {
                return Err(SessionError::corrupted(format!(
                    "unknown session entry type {other}"
                )));
            }
        }
        Ok(())
    }
}

/// Validate append order and return the active leaf after replay.
pub fn validate_session_entries(
    entries: &[StoredSessionEntry],
) -> Result<Option<String>, SessionError> {
    let mut seen = std::collections::HashSet::new();
    let mut active_leaf_id = None;
    for entry in entries {
        if !seen.insert(entry.id.clone()) {
            return Err(SessionError::corrupted(
                "session transcript contains duplicate entry id",
            ));
        }
        if let Some(parent) = &entry.parent_id
            && !seen.contains(parent)
        {
            return Err(SessionError::corrupted(
                "session transcript contains dangling parent reference",
            ));
        }
        active_leaf_id = match entry.leaf_target_id() {
            Some(Some(target)) if !seen.contains(target) => {
                return Err(SessionError::corrupted(
                    "session transcript contains dangling leaf target",
                ));
            }
            Some(target) => target.map(str::to_string),
            None => Some(entry.id.clone()),
        };
    }
    Ok(active_leaf_id)
}

#[async_trait]
pub trait SessionReader: Send + Sync {
    async fn get_metadata_json(&self) -> Result<Value, SessionError>;
    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError>;
    async fn get_entry(&self, id: &str) -> Result<Option<StoredSessionEntry>, SessionError>;
    async fn get_entries(&self) -> Result<Vec<StoredSessionEntry>, SessionError>;
    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError>;
    async fn find_entries(&self, entry_type: &str)
    -> Result<Vec<StoredSessionEntry>, SessionError>;
    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError>;

    /// Return one extension's entries on the selected branch in root-to-leaf
    /// replay order. `None` selects the store's current active leaf.
    async fn get_extension_entries(
        &self,
        extension_id: &str,
        leaf_id: Option<&str>,
    ) -> Result<Vec<StoredSessionEntry>, SessionError> {
        let selected_leaf = match leaf_id {
            Some(id) => Some(id.to_string()),
            None => self.get_leaf_id().await?,
        };
        let path = self.get_path_to_root(selected_leaf.as_deref()).await?;
        let mut entries = Vec::new();
        for stored in path {
            let Some(extension) = stored.extension_payload()? else {
                continue;
            };
            if extension.extension_id == extension_id {
                entries.push(stored);
            }
        }
        Ok(entries)
    }
}

#[async_trait]
pub trait SessionStore: SessionReader {
    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError>;
    async fn create_entry_id(&self) -> Result<String, SessionError>;
    /// Atomically append a sequence of entries in the provided order. Either
    /// every entry becomes visible or none of them does.
    async fn append_entries(&self, entries: Vec<StoredSessionEntry>) -> Result<(), SessionError>;

    /// Persist or clear a non-secret client binding. Backends that cannot
    /// support binding updates must fail closed instead of silently succeeding.
    async fn set_binding(&self, _binding: Option<SessionBinding>) -> Result<(), SessionError> {
        Err(SessionError::new(
            SessionErrorCode::StorageFailure,
            "session store does not support binding updates",
        ))
    }

    async fn append_entry(&self, entry: StoredSessionEntry) -> Result<(), SessionError> {
        self.append_entries(vec![entry]).await
    }
}
