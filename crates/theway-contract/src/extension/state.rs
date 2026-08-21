use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::is_valid_extension_id;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDurableEntryKind {
    StateMutation,
    CustomEvent,
    ModelContext,
    StateMigration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionStateMutation {
    Set { value: Value },
    Delete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionModelContextPlacement {
    SystemPromptSection,
    Message,
}

/// Domain payload persisted inside an opaque extension session entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExtensionDurableEntryPayload {
    StateMutation {
        key: String,
        mutation: ExtensionStateMutation,
    },
    CustomEvent {
        event_id: String,
        custom_type: String,
        payload: Value,
    },
    ModelContext {
        context_id: String,
        placement: ExtensionModelContextPlacement,
        content: Value,
    },
    StateMigration {
        from_schema_version: u32,
        to_schema_version: u32,
    },
}

impl ExtensionDurableEntryPayload {
    pub const fn kind(&self) -> ExtensionDurableEntryKind {
        match self {
            Self::StateMutation { .. } => ExtensionDurableEntryKind::StateMutation,
            Self::CustomEvent { .. } => ExtensionDurableEntryKind::CustomEvent,
            Self::ModelContext { .. } => ExtensionDurableEntryKind::ModelContext,
            Self::StateMigration { .. } => ExtensionDurableEntryKind::StateMigration,
        }
    }
}

/// Extension-owned entry stored on the active session branch. The
/// surrounding session record supplies append identity, parent, and timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionDurableEntry {
    pub extension_id: String,
    pub state_schema_version: u32,
    pub origin_sequence: u64,
    pub entry: ExtensionDurableEntryPayload,
}

impl ExtensionDurableEntry {
    pub fn validate(&self) -> Result<(), ExtensionStateValidationError> {
        if !is_valid_extension_id(&self.extension_id) {
            return Err(ExtensionStateValidationError::InvalidExtensionId);
        }
        if self.state_schema_version == 0 {
            return Err(ExtensionStateValidationError::InvalidStateSchema);
        }
        if self.origin_sequence == 0 {
            return Err(ExtensionStateValidationError::InvalidOriginSequence);
        }
        match &self.entry {
            ExtensionDurableEntryPayload::StateMutation { key, .. } => {
                if !is_valid_local_identifier(key, 256) {
                    return Err(ExtensionStateValidationError::InvalidStateKey);
                }
            }
            ExtensionDurableEntryPayload::CustomEvent {
                event_id,
                custom_type,
                ..
            } => {
                if !is_valid_local_identifier(event_id, 256) {
                    return Err(ExtensionStateValidationError::InvalidEventId);
                }
                if !is_valid_type_name(custom_type) {
                    return Err(ExtensionStateValidationError::InvalidCustomType);
                }
            }
            ExtensionDurableEntryPayload::ModelContext {
                context_id,
                placement,
                content,
            } => {
                if !is_valid_local_identifier(context_id, 256) {
                    return Err(ExtensionStateValidationError::InvalidContextId);
                }
                let valid_content = match placement {
                    ExtensionModelContextPlacement::SystemPromptSection => content.is_string(),
                    ExtensionModelContextPlacement::Message => content.is_object(),
                };
                if !valid_content {
                    return Err(ExtensionStateValidationError::InvalidModelContext);
                }
            }
            ExtensionDurableEntryPayload::StateMigration {
                from_schema_version,
                to_schema_version,
            } => {
                if *from_schema_version == 0
                    || *from_schema_version >= *to_schema_version
                    || *to_schema_version != self.state_schema_version
                {
                    return Err(ExtensionStateValidationError::InvalidMigration);
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionStateValidationError {
    #[error("extension durable entry has an invalid extension id")]
    InvalidExtensionId,
    #[error("extension durable entry state schema must be greater than zero")]
    InvalidStateSchema,
    #[error("extension durable entry origin sequence must be greater than zero")]
    InvalidOriginSequence,
    #[error("extension state key is invalid")]
    InvalidStateKey,
    #[error("extension custom event id is invalid")]
    InvalidEventId,
    #[error("extension custom event type is invalid")]
    InvalidCustomType,
    #[error("extension model context id is invalid")]
    InvalidContextId,
    #[error("extension model context content does not match its placement")]
    InvalidModelContext,
    #[error("extension state migration versions are invalid")]
    InvalidMigration,
}

fn is_valid_local_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.chars().all(|character| !character.is_control())
}

fn is_valid_type_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with(['.', '-', '_'])
        && !value.ends_with(['.', '-', '_'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}
