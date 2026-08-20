use std::borrow::Cow;
use std::fmt;
use std::path::{Component, Path};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use super::{first_duplicate, is_valid_extension_id};

/// First versioned runtime-extension ABI. The legacy compaction-only format is
/// intentionally outside this version line.
pub const RUNTIME_EXTENSION_ABI_MAJOR: u16 = 2;

/// ABI major declared by an extension package.
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
#[serde(transparent)]
pub struct ExtensionAbiMajor(pub u16);

impl ExtensionAbiMajor {
    pub const V2: Self = Self(RUNTIME_EXTENSION_ABI_MAJOR);

    pub const fn is_supported(self) -> bool {
        self.0 == RUNTIME_EXTENSION_ABI_MAJOR
    }
}

/// Lifetime boundary for extension instances and their owned effects.
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
pub enum ExtensionScope {
    Process,
    Session,
    Run,
    Request,
}

/// Discovery layer attached by the host after reading a package manifest.
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
pub enum ExtensionSourceLayer {
    Global,
    Project,
}

/// Capability name declared by an extension manifest.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExtensionPermission {
    SessionWrite,
    ToolsRegister,
    ToolsOverride,
    CommandsRegister,
    ProvidersRegister,
    ClientContribute,
    WorkspaceRead,
    WorkspaceWrite,
    ProcessSpawn,
    NetworkConnect,
    ProviderRaw,
    SecretsRead(String),
}

impl ExtensionPermission {
    pub fn canonical_name(&self) -> Cow<'_, str> {
        match self {
            Self::SessionWrite => Cow::Borrowed("session.write"),
            Self::ToolsRegister => Cow::Borrowed("tools.register"),
            Self::ToolsOverride => Cow::Borrowed("tools.override"),
            Self::CommandsRegister => Cow::Borrowed("commands.register"),
            Self::ProvidersRegister => Cow::Borrowed("providers.register"),
            Self::ClientContribute => Cow::Borrowed("client.contribute"),
            Self::WorkspaceRead => Cow::Borrowed("workspace.read"),
            Self::WorkspaceWrite => Cow::Borrowed("workspace.write"),
            Self::ProcessSpawn => Cow::Borrowed("process.spawn"),
            Self::NetworkConnect => Cow::Borrowed("network.connect"),
            Self::ProviderRaw => Cow::Borrowed("provider.raw"),
            Self::SecretsRead(name) => Cow::Owned(format!("secrets.read:{name}")),
        }
    }

    pub fn secret_name(&self) -> Option<&str> {
        match self {
            Self::SecretsRead(name) => Some(name),
            _ => None,
        }
    }
}

impl fmt::Display for ExtensionPermission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.canonical_name())
    }
}

impl FromStr for ExtensionPermission {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let permission = match value {
            "session.write" => Self::SessionWrite,
            "tools.register" => Self::ToolsRegister,
            "tools.override" => Self::ToolsOverride,
            "commands.register" => Self::CommandsRegister,
            "providers.register" => Self::ProvidersRegister,
            "client.contribute" => Self::ClientContribute,
            "workspace.read" => Self::WorkspaceRead,
            "workspace.write" => Self::WorkspaceWrite,
            "process.spawn" => Self::ProcessSpawn,
            "network.connect" => Self::NetworkConnect,
            "provider.raw" => Self::ProviderRaw,
            _ => {
                let Some(name) = value.strip_prefix("secrets.read:") else {
                    return Err(format!("unknown extension permission {value}"));
                };
                if !is_valid_permission_segment(name) {
                    return Err("secret permission must name one concrete secret".into());
                }
                Self::SecretsRead(name.to_string())
            }
        };
        Ok(permission)
    }
}

impl Serialize for ExtensionPermission {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.canonical_name())
    }
}

impl<'de> Deserialize<'de> for ExtensionPermission {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl schemars::JsonSchema for ExtensionPermission {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("ExtensionPermission")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                {
                    "enum": [
                        "session.write",
                        "tools.register",
                        "tools.override",
                        "commands.register",
                        "providers.register",
                        "client.contribute",
                        "workspace.read",
                        "workspace.write",
                        "process.spawn",
                        "network.connect",
                        "provider.raw"
                    ]
                },
                {
                    "type": "string",
                    "pattern": "^secrets\\.read:[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$"
                }
            ]
        })
    }
}

/// Strict on-disk package manifest for an ABI v2 runtime extension.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtensionPackageManifest {
    pub id: String,
    pub version: String,
    pub abi: ExtensionAbiMajor,
    pub entry: String,
    #[serde(default)]
    pub priority: i32,
    pub scope: ExtensionScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_schema: Option<u32>,
    #[serde(default)]
    pub permissions: Vec<ExtensionPermission>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_permissions: Vec<ExtensionPermission>,
}

impl ExtensionPackageManifest {
    pub fn validate(&self) -> Result<(), ExtensionManifestError> {
        if !is_valid_extension_id(&self.id) {
            return Err(ExtensionManifestError::InvalidId);
        }
        semver::Version::parse(&self.version)
            .map_err(|_| ExtensionManifestError::InvalidVersion)?;
        if !self.abi.is_supported() {
            return Err(ExtensionManifestError::UnsupportedAbi(self.abi.0));
        }
        if !is_safe_relative_entry(&self.entry) {
            return Err(ExtensionManifestError::InvalidEntry);
        }
        if self.state_schema == Some(0) {
            return Err(ExtensionManifestError::InvalidStateSchema);
        }
        if let Some(permission) = first_duplicate(&self.permissions) {
            return Err(ExtensionManifestError::DuplicatePermission(
                permission.to_string(),
            ));
        }
        if let Some(permission) = first_duplicate(&self.optional_permissions) {
            return Err(ExtensionManifestError::DuplicatePermission(
                permission.to_string(),
            ));
        }
        if let Some(permission) = self
            .permissions
            .iter()
            .find(|permission| self.optional_permissions.contains(permission))
        {
            return Err(ExtensionManifestError::RequiredOptionalOverlap(
                permission.to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionManifestError {
    #[error("extension id must be 1-64 lowercase ASCII letters, digits, or single hyphens")]
    InvalidId,
    #[error("extension version must be valid semantic versioning")]
    InvalidVersion,
    #[error("extension ABI major {0} is not supported")]
    UnsupportedAbi(u16),
    #[error("extension entry must be a non-empty relative path without parent traversal")]
    InvalidEntry,
    #[error("extension state schema must be greater than zero")]
    InvalidStateSchema,
    #[error("extension permission {0} is declared more than once")]
    DuplicatePermission(String),
    #[error("extension permission {0} cannot be both required and optional")]
    RequiredOptionalOverlap(String),
}

fn is_safe_relative_entry(entry: &str) -> bool {
    if entry.is_empty() {
        return false;
    }
    let path = Path::new(entry);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        && path
            .components()
            .any(|component| matches!(component, Component::Normal(_)))
}

fn is_valid_permission_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with(['.', '-', '_'])
        && !value.ends_with(['.', '-', '_'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}
