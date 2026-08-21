use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ExtensionLifecycleEvent, ExtensionPermission, ExtensionScope, ExtensionSourceLayer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCatalogStatus {
    Effective,
    Shadowed,
    Rejected,
    Blocked,
    Disabled,
    Faulted,
}

/// Client-neutral catalog record. It intentionally omits executable engine
/// values and secret-bearing provider configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionCatalogEntry {
    pub extension_id: String,
    pub version: String,
    pub source: ExtensionSourceLayer,
    pub scope: ExtensionScope,
    pub priority: i32,
    pub status: ExtensionCatalogStatus,
    #[serde(default)]
    pub permissions: Vec<ExtensionPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<ExtensionDiagnosticCode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionDiagnosticCode {
    ManifestInvalid,
    Shadowed,
    TrustRequired,
    PermissionDenied,
    LoadFailed,
    HookFailed,
    HookTimedOut,
    Cancelled,
    ResourceLimit,
    CircuitOpened,
    Unloaded,
    ReloadPending,
    StateMigrationFailed,
    PersistenceFailed,
    QueueOverflow,
    LifecycleStatus,
    ContractViolation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtensionDiagnosticSensitivity {
    Public,
    Sensitive,
}

/// Structured diagnostic. Sensitive detail values are discarded when added;
/// serialization exposes only their field names in `redactedFields`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionDiagnostic {
    pub extension_id: String,
    pub code: ExtensionDiagnosticCode,
    pub severity: ExtensionDiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<ExtensionLifecycleEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub redacted_fields: BTreeSet<String>,
}

impl ExtensionDiagnostic {
    pub fn new(
        extension_id: impl Into<String>,
        code: ExtensionDiagnosticCode,
        severity: ExtensionDiagnosticSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            extension_id: extension_id.into(),
            code,
            severity,
            message: message.into(),
            session_id: None,
            event: None,
            sequence: None,
            details: BTreeMap::new(),
            redacted_fields: BTreeSet::new(),
        }
    }

    pub fn add_detail(
        &mut self,
        key: impl Into<String>,
        value: Value,
        sensitivity: ExtensionDiagnosticSensitivity,
    ) {
        let key = key.into();
        match sensitivity {
            ExtensionDiagnosticSensitivity::Public => {
                self.redacted_fields.remove(&key);
                self.details.insert(key, value);
            }
            ExtensionDiagnosticSensitivity::Sensitive => {
                self.details.remove(&key);
                self.redacted_fields.insert(key);
            }
        }
    }
}
