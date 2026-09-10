//! Argument shapes of the capability broker operations. `camelCase` where the
//! JS host API uses camelCase; every struct denies unknown fields, so a typo in
//! a script argument surfaces as `invalid_arguments` instead of being ignored.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CapabilityArguments {
    pub(super) permission: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ServiceArguments {
    pub(super) name: String,
    #[serde(default)]
    pub(super) value: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NativeArguments {
    pub(super) name: String,
    #[serde(default)]
    pub(super) args: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublishArguments {
    pub(super) event: String,
    #[serde(default)]
    pub(super) payload: Option<Value>,
    #[serde(default)]
    pub(super) mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadArguments {
    pub(super) path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WriteArguments {
    pub(super) path: String,
    pub(super) content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessArguments {
    pub(super) argv: Vec<String>,
    #[serde(default)]
    pub(super) timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NetworkArguments {
    pub(super) url: String,
    #[serde(default)]
    pub(super) method: Option<String>,
    #[serde(default)]
    pub(super) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(super) body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SecretArguments {
    pub(super) name: String,
}
