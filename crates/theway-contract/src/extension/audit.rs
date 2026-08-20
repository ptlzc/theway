use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::ExtensionPermission;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionAuditOperation {
    TrustChanged,
    WorkspaceRead,
    WorkspaceWrite,
    ProcessSpawn,
    NetworkConnect,
    SecretRead,
    ProviderRawRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionAuditOutcome {
    Allowed,
    Denied,
    Succeeded,
    Failed,
    Cancelled,
}

/// Redacted security audit record. Targets are bounded public identifiers;
/// secret values, request bodies, command arguments, model content, and
/// extension-private state have no fields in this contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionAuditEvent {
    pub timestamp: String,
    pub extension_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub operation: ExtensionAuditOperation,
    pub outcome: ExtensionAuditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<ExtensionPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub redacted_fields: BTreeSet<String>,
}
