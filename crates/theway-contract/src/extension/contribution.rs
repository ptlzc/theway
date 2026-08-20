use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::{ExtensionScope, is_valid_extension_id};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionNoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionCommandDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    pub argument_schema: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionCommandOutcome {
    Success {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
    },
    Rejected {
        code: String,
        message: String,
    },
    Cancelled {
        code: String,
        message: String,
    },
}

/// Declarative contribution understood independently by each client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExtensionClientContributionData {
    Notification {
        level: ExtensionNoticeLevel,
        title: String,
        body: String,
    },
    StatusItem {
        label: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    Command {
        command: ExtensionCommandDescriptor,
    },
    DetailPanel {
        title: String,
        data: Value,
    },
    FormAction {
        title: String,
        schema: Value,
        submit_command: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionClientContribution {
    pub contribution_id: String,
    pub extension_id: String,
    pub scope: ExtensionScope,
    pub contribution: ExtensionClientContributionData,
}

impl ExtensionClientContribution {
    pub fn validate(&self) -> Result<(), ExtensionContributionError> {
        if !is_valid_extension_id(&self.extension_id) {
            return Err(ExtensionContributionError::InvalidExtensionId);
        }
        if !is_valid_local_id(&self.contribution_id) {
            return Err(ExtensionContributionError::InvalidContributionId);
        }
        match &self.contribution {
            ExtensionClientContributionData::Command { command } => command.validate(),
            ExtensionClientContributionData::DetailPanel { data, .. } => {
                if !data.is_object() && !data.is_array() {
                    return Err(ExtensionContributionError::InvalidDataSchema);
                }
                Ok(())
            }
            ExtensionClientContributionData::FormAction {
                schema,
                submit_command,
                ..
            } => {
                if !schema.is_object() || !is_valid_command_name(submit_command) {
                    return Err(ExtensionContributionError::InvalidDataSchema);
                }
                Ok(())
            }
            ExtensionClientContributionData::Notification { .. }
            | ExtensionClientContributionData::StatusItem { .. } => Ok(()),
        }
    }
}

impl ExtensionCommandDescriptor {
    pub fn validate(&self) -> Result<(), ExtensionContributionError> {
        if !is_valid_command_name(&self.name) {
            return Err(ExtensionContributionError::InvalidCommandName);
        }
        if self.label.trim().is_empty() || self.description.trim().is_empty() {
            return Err(ExtensionContributionError::InvalidCommandMetadata);
        }
        if !self.argument_schema.is_object() {
            return Err(ExtensionContributionError::InvalidDataSchema);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionContributionError {
    #[error("extension contribution has an invalid extension id")]
    InvalidExtensionId,
    #[error("extension contribution id is invalid")]
    InvalidContributionId,
    #[error("extension command name is invalid")]
    InvalidCommandName,
    #[error("extension command label and description must not be empty")]
    InvalidCommandMetadata,
    #[error("extension contribution data or schema has an invalid shape")]
    InvalidDataSchema,
}

fn is_valid_local_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn is_valid_command_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
