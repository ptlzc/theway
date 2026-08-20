// Client-neutral runtime-extension wire models. Contribution kinds and
// payloads stay open so an older client can ignore a contribution introduced
// by a newer daemon. Executable engine values never cross this boundary.

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionSnapshot {
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub reload_pending: bool,
    #[serde(default)]
    pub catalog: Vec<WireExtensionCatalogEntry>,
    #[serde(default)]
    pub diagnostics: Vec<WireExtensionDiagnostic>,
    #[serde(default)]
    pub commands: Vec<WireExtensionCommandDescriptor>,
    #[serde(default)]
    pub contributions: Vec<WireExtensionContribution>,
}

impl WireExtensionSnapshot {
    pub fn is_empty(&self) -> bool {
        self.revision == 0
            && !self.reload_pending
            && self.catalog.is_empty()
            && self.diagnostics.is_empty()
            && self.commands.is_empty()
            && self.contributions.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionCatalogEntry {
    pub extension_id: String,
    pub version: String,
    /// Optional for forward/backward compatibility with pre-ABI catalogs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi_major: Option<u32>,
    pub source: String,
    pub scope: String,
    pub priority: i32,
    pub status: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionDiagnostic {
    pub extension_id: String,
    pub code: String,
    pub severity: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub details: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionCommandDescriptor {
    pub extension_id: String,
    pub name: String,
    pub label: String,
    pub description: String,
    pub argument_schema: serde_json::Value,
}

/// Open contribution envelope. `kind` is intentionally a string instead of
/// an enum; clients render supported values and skip all others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionContribution {
    pub contribution_id: String,
    pub extension_id: String,
    pub scope: String,
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionCommandOutcome {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionReloadResult {
    pub status: String,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionTrustRequest {
    /// `project` trusts every project package with its declared permission set;
    /// `package` targets `extension_id` and exact package content.
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_id: Option<String>,
    pub decision: String,
    #[serde(default)]
    pub granted_permissions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireExtensionTrustResult {
    pub accepted: bool,
    pub reload: WireExtensionReloadResult,
}
