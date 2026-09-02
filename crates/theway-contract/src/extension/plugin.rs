use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Registration descriptor for a plugin-declared action. This is the
/// declarative form submitted through the `registerAction` bridge, distinct
/// from the runtime [`ExtensionAction`](super::ExtensionAction) returned by a
/// hook invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginActionRegistration {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub input_schema: Value,
}

/// Registration descriptor for a plugin-provided service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceRegistration {
    pub name: String,
}
