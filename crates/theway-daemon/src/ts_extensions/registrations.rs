use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionClientContribution, ExtensionCommandDescriptor, ExtensionPermission, ExtensionScope,
    PluginActionRegistration, ServiceRegistration,
};
use theway_llm_provider::{Api, InputModality, Model, ModelCost, Provider, Tool};

use super::effects::EffectKind;

const MAX_REGISTRATIONS: usize = 128;
const MAX_TEXT_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct EffectRegistration {
    pub registration_id: u64,
    pub sequence: u64,
    pub value: OwnedRegistration,
}

impl EffectRegistration {
    pub fn kind(&self) -> EffectKind {
        self.value.kind()
    }

    pub fn scope(&self) -> ExtensionScope {
        self.value.scope()
    }

    pub fn conflict_key(&self) -> String {
        self.value.conflict_key()
    }

    pub fn requests_override(&self) -> bool {
        self.value.requests_override()
    }
}

#[derive(Clone, Debug)]
pub enum OwnedRegistration {
    Hook(HookEffectRegistration),
    Tool(ToolRegistration),
    Command(CommandRegistration),
    Provider(ProviderRegistration),
    PromptSection(PromptSectionRegistration),
    RequestPolicy(RequestPolicyRegistration),
    Contribution(ExtensionClientContribution),
    Action(PluginActionRegistration),
    Service(ServiceRegistration),
}

impl OwnedRegistration {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Hook(_) => EffectKind::Hook,
            Self::Tool(_) => EffectKind::Tool,
            Self::Command(_) => EffectKind::Command,
            Self::Provider(_) => EffectKind::Provider,
            Self::PromptSection(_) => EffectKind::PromptSection,
            Self::RequestPolicy(_) => EffectKind::RequestPolicy,
            Self::Contribution(_) => EffectKind::Contribution,
            Self::Action(_) => EffectKind::Action,
            Self::Service(_) => EffectKind::Service,
        }
    }

    pub fn scope(&self) -> ExtensionScope {
        match self {
            Self::Hook(value) => value.scope,
            Self::Tool(value) => value.scope,
            Self::Command(value) => value.scope,
            Self::Provider(value) => value.scope,
            Self::PromptSection(value) => value.scope,
            Self::RequestPolicy(value) => value.scope,
            Self::Contribution(value) => value.scope,
            Self::Action(_) | Self::Service(_) => ExtensionScope::Session,
        }
    }

    pub fn conflict_key(&self) -> String {
        match self {
            Self::Hook(value) => value.key.clone(),
            Self::Tool(value) => value.definition.name.clone(),
            Self::Command(value) => value.command.name.clone(),
            Self::Provider(value) => value.provider_id.clone(),
            Self::PromptSection(value) => value.section_id.clone(),
            Self::RequestPolicy(value) => value.policy_id.clone(),
            Self::Contribution(value) => value.contribution_id.clone(),
            Self::Action(value) => value.name.clone(),
            Self::Service(value) => value.name.clone(),
        }
    }

    pub fn requests_override(&self) -> bool {
        matches!(self, Self::Tool(value) if value.override_existing)
    }
}

#[derive(Clone, Debug)]
pub struct HookEffectRegistration {
    pub key: String,
    pub scope: ExtensionScope,
}

#[derive(Clone, Debug)]
pub struct ToolRegistration {
    pub definition: Tool,
    pub label: String,
    pub result_schema: Option<Value>,
    pub permission: ToolPermission,
    pub scope: ExtensionScope,
    pub override_existing: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    #[default]
    Allow,
    Prompt,
    Block,
}

#[derive(Clone, Debug)]
pub struct CommandRegistration {
    pub command: ExtensionCommandDescriptor,
    pub availability: RegistrationPredicate,
    pub scope: ExtensionScope,
}

#[derive(Clone, Debug)]
pub struct ProviderRegistration {
    pub provider_id: String,
    pub base_url: String,
    pub format: ProviderWireFormat,
    pub credential_ref: Option<String>,
    pub models: Vec<ProviderModelRegistration>,
    pub scope: ExtensionScope,
}

impl ProviderRegistration {
    pub fn models(&self) -> Vec<Model> {
        self.models
            .iter()
            .map(|model| Model {
                id: model.id.clone(),
                name: model.name.clone(),
                api: Api(self.format.api_id().into()),
                provider: Provider(self.provider_id.clone()),
                base_url: self.base_url.clone(),
                reasoning: model.reasoning,
                thinking_level_map: None,
                input: model.input.clone(),
                cost: ModelCost::default(),
                context_window: model.context_window,
                max_tokens: model.max_tokens,
                headers: None,
                compat: None,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWireFormat {
    OpenaiChatCompletions,
    OpenaiResponses,
    AnthropicMessages,
}

impl ProviderWireFormat {
    fn api_id(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "openai-completions",
            Self::OpenaiResponses => "openai-responses",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModelRegistration {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default = "text_input")]
    pub input: Vec<InputModality>,
    pub context_window: u32,
    pub max_tokens: u32,
}

#[derive(Clone, Debug)]
pub struct PromptSectionRegistration {
    pub section_id: String,
    pub text: String,
    pub priority: i32,
    pub predicate: RegistrationPredicate,
    pub scope: ExtensionScope,
}

#[derive(Clone, Debug)]
pub struct RequestPolicyRegistration {
    pub policy_id: String,
    pub priority: i32,
    pub predicate: RegistrationPredicate,
    pub scope: ExtensionScope,
    pub granted_permissions: BTreeSet<ExtensionPermission>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrationPredicate {
    #[serde(default)]
    pub providers: BTreeSet<String>,
    #[serde(default)]
    pub models: BTreeSet<String>,
    #[serde(default)]
    pub requires_interactive_client: bool,
}

impl RegistrationPredicate {
    pub fn matches(&self, provider: &str, model: &str, interactive: bool) -> bool {
        (self.providers.is_empty() || self.providers.contains(provider))
            && (self.models.is_empty() || self.models.contains(model))
            && (!self.requires_interactive_client || interactive)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawEffectRegistration {
    registration_id: u64,
    kind: RawEffectKind,
    descriptor: Value,
    sequence: u64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawEffectKind {
    Tool,
    Command,
    Provider,
    PromptSection,
    RequestPolicy,
    Contribution,
    Action,
    Service,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawToolRegistration {
    name: String,
    label: String,
    description: String,
    input_schema: Value,
    #[serde(default)]
    result_schema: Option<Value>,
    #[serde(default)]
    permission: ToolPermission,
    #[serde(default = "session_scope")]
    scope: ExtensionScope,
    #[serde(default, rename = "override")]
    override_existing: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawCommandRegistration {
    name: String,
    label: String,
    description: String,
    argument_schema: Value,
    #[serde(default)]
    availability: RegistrationPredicate,
    #[serde(default = "session_scope")]
    scope: ExtensionScope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawProviderRegistration {
    provider_id: String,
    base_url: String,
    format: ProviderWireFormat,
    #[serde(default)]
    credential_ref: Option<String>,
    models: Vec<ProviderModelRegistration>,
    #[serde(default = "session_scope")]
    scope: ExtensionScope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawPromptSectionRegistration {
    section_id: String,
    text: String,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    predicate: RegistrationPredicate,
    #[serde(default = "session_scope")]
    scope: ExtensionScope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRequestPolicyRegistration {
    policy_id: String,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    predicate: RegistrationPredicate,
    #[serde(default = "session_scope")]
    scope: ExtensionScope,
}

pub(super) struct ValidatedEffectRegistrations {
    pub registrations: Vec<EffectRegistration>,
    pub errors: Vec<String>,
}

pub(super) fn validate_effect_registrations(
    metadata: &Value,
    extension_id: &str,
    manifest_scope: ExtensionScope,
    granted: &BTreeSet<ExtensionPermission>,
) -> Result<ValidatedEffectRegistrations, String> {
    let raw: Vec<Value> = serde_json::from_value(
        metadata
            .get("effects")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )
    .map_err(|error| format!("extension effects are invalid: {error}"))?;
    if raw.len() > MAX_REGISTRATIONS {
        return Err("extension effect count exceeds the configured limit".into());
    }
    let mut ids = BTreeSet::new();
    let mut effects = Vec::with_capacity(raw.len());
    let mut errors = Vec::new();
    for raw_item in raw {
        let item: RawEffectRegistration = match serde_json::from_value(raw_item) {
            Ok(item) => item,
            Err(error) => {
                errors.push(format!("extension effect registration is invalid: {error}"));
                continue;
            }
        };
        if !ids.insert(item.registration_id) {
            errors.push("extension effect registration ids must be unique".into());
            continue;
        }
        let value = match decode_registration(item.kind, item.descriptor, extension_id, granted)
            .and_then(|value| {
                validate_scope(manifest_scope, value.scope())?;
                require_permission(&value, granted)?;
                Ok(value)
            }) {
            Ok(value) => value,
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        effects.push(EffectRegistration {
            registration_id: item.registration_id,
            sequence: item.sequence,
            value,
        });
    }
    effects.sort_by_key(|effect| effect.sequence);
    Ok(ValidatedEffectRegistrations {
        registrations: effects,
        errors,
    })
}

fn decode_registration(
    kind: RawEffectKind,
    descriptor: Value,
    extension_id: &str,
    granted: &BTreeSet<ExtensionPermission>,
) -> Result<OwnedRegistration, String> {
    match kind {
        RawEffectKind::Tool => {
            let raw: RawToolRegistration = decode(descriptor, "tool")?;
            validate_name(&raw.name, "tool")?;
            validate_text(&raw.label, "tool label")?;
            validate_text(&raw.description, "tool description")?;
            validate_schema(&raw.input_schema, "tool input")?;
            if let Some(schema) = &raw.result_schema {
                validate_schema(schema, "tool result")?;
            }
            Ok(OwnedRegistration::Tool(ToolRegistration {
                definition: Tool {
                    name: raw.name,
                    description: raw.description,
                    parameters: raw.input_schema,
                },
                label: raw.label,
                result_schema: raw.result_schema,
                permission: raw.permission,
                scope: raw.scope,
                override_existing: raw.override_existing,
            }))
        }
        RawEffectKind::Command => {
            let raw: RawCommandRegistration = decode(descriptor, "command")?;
            let command = ExtensionCommandDescriptor {
                name: raw.name,
                label: raw.label,
                description: raw.description,
                argument_schema: raw.argument_schema,
            };
            command.validate().map_err(|error| error.to_string())?;
            Ok(OwnedRegistration::Command(CommandRegistration {
                command,
                availability: raw.availability,
                scope: raw.scope,
            }))
        }
        RawEffectKind::Provider => {
            let raw: RawProviderRegistration = decode(descriptor, "provider")?;
            validate_name(&raw.provider_id, "provider")?;
            let url = reqwest::Url::parse(&raw.base_url)
                .map_err(|_| "provider baseUrl must be an absolute http(s) URL")?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err("provider baseUrl must be an absolute http(s) URL".into());
            }
            if raw.models.is_empty() || raw.models.len() > 256 {
                return Err("provider models must contain 1-256 entries".into());
            }
            let mut model_ids = BTreeSet::new();
            for model in &raw.models {
                validate_name(&model.id, "model")?;
                validate_text(&model.name, "model name")?;
                if !model_ids.insert(model.id.as_str())
                    || model.context_window == 0
                    || model.max_tokens == 0
                    || model.max_tokens > model.context_window
                {
                    return Err("provider model metadata is invalid".into());
                }
            }
            if raw
                .credential_ref
                .as_deref()
                .is_some_and(|value| validate_local_id(value).is_err())
            {
                return Err("provider credentialRef is invalid".into());
            }
            Ok(OwnedRegistration::Provider(ProviderRegistration {
                provider_id: raw.provider_id,
                base_url: raw.base_url,
                format: raw.format,
                credential_ref: raw.credential_ref,
                models: raw.models,
                scope: raw.scope,
            }))
        }
        RawEffectKind::PromptSection => {
            let raw: RawPromptSectionRegistration = decode(descriptor, "prompt section")?;
            validate_local_id(&raw.section_id)?;
            validate_text(&raw.text, "prompt section text")?;
            Ok(OwnedRegistration::PromptSection(
                PromptSectionRegistration {
                    section_id: raw.section_id,
                    text: raw.text,
                    priority: raw.priority,
                    predicate: raw.predicate,
                    scope: raw.scope,
                },
            ))
        }
        RawEffectKind::RequestPolicy => {
            let raw: RawRequestPolicyRegistration = decode(descriptor, "request policy")?;
            validate_local_id(&raw.policy_id)?;
            Ok(OwnedRegistration::RequestPolicy(
                RequestPolicyRegistration {
                    policy_id: raw.policy_id,
                    priority: raw.priority,
                    predicate: raw.predicate,
                    scope: raw.scope,
                    granted_permissions: granted.clone(),
                },
            ))
        }
        RawEffectKind::Contribution => {
            let contribution: ExtensionClientContribution =
                decode(descriptor, "client contribution")?;
            if contribution.extension_id != extension_id {
                return Err("client contribution owner does not match the package".into());
            }
            contribution.validate().map_err(|error| error.to_string())?;
            Ok(OwnedRegistration::Contribution(contribution))
        }
        RawEffectKind::Action => {
            let raw: PluginActionRegistration = decode(descriptor, "action")?;
            validate_name(&raw.name, "action")?;
            validate_schema(&raw.input_schema, "action input")?;
            Ok(OwnedRegistration::Action(PluginActionRegistration {
                name: raw.name,
                description: raw.description,
                input_schema: raw.input_schema,
            }))
        }
        RawEffectKind::Service => {
            let raw: ServiceRegistration = decode(descriptor, "service")?;
            validate_name(&raw.name, "service")?;
            Ok(OwnedRegistration::Service(ServiceRegistration {
                name: raw.name,
            }))
        }
    }
}

fn require_permission(
    registration: &OwnedRegistration,
    granted: &BTreeSet<ExtensionPermission>,
) -> Result<(), String> {
    let mut required = match registration {
        OwnedRegistration::Tool(value) if value.override_existing => vec![
            ExtensionPermission::ToolsRegister,
            ExtensionPermission::ToolsOverride,
        ],
        OwnedRegistration::Tool(_) => vec![ExtensionPermission::ToolsRegister],
        OwnedRegistration::Command(_) => vec![ExtensionPermission::CommandsRegister],
        OwnedRegistration::Provider(_) => vec![ExtensionPermission::ProvidersRegister],
        OwnedRegistration::Contribution(_) => vec![ExtensionPermission::ClientContribute],
        OwnedRegistration::Action(_) => vec![ExtensionPermission::ActionsRegister],
        OwnedRegistration::Service(_) => vec![ExtensionPermission::ServicesProvide],
        OwnedRegistration::Hook(_)
        | OwnedRegistration::PromptSection(_)
        | OwnedRegistration::RequestPolicy(_) => Vec::new(),
    };
    if let OwnedRegistration::Provider(provider) = registration
        && let Some(name) = &provider.credential_ref
    {
        required.push(ExtensionPermission::SecretsRead(name.clone()));
    }
    if let Some(missing) = required
        .iter()
        .find(|permission| !granted.contains(permission))
    {
        return Err(format!(
            "{:?} registration requires the {} capability",
            registration.kind(),
            missing.canonical_name()
        ));
    }
    Ok(())
}

pub fn hook_effect_registration(
    registration_id: u64,
    sequence: u64,
    key: String,
    scope: ExtensionScope,
) -> EffectRegistration {
    EffectRegistration {
        registration_id,
        sequence,
        value: OwnedRegistration::Hook(HookEffectRegistration { key, scope }),
    }
}

fn validate_scope(manifest: ExtensionScope, effect: ExtensionScope) -> Result<(), String> {
    let rank = |scope| match scope {
        ExtensionScope::Process => 0,
        ExtensionScope::Session => 1,
        ExtensionScope::Run => 2,
        ExtensionScope::Request => 3,
    };
    if rank(effect) < rank(manifest) {
        return Err("registration scope cannot be wider than the package scope".into());
    }
    Ok(())
}

fn validate_name(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(format!("{label} name is invalid"));
    }
    Ok(())
}

fn validate_local_id(value: &str) -> Result<(), String> {
    validate_name(value, "registration")
}

fn validate_text(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(format!("{label} is empty or too large"));
    }
    Ok(())
}

fn validate_schema(value: &Value, label: &str) -> Result<(), String> {
    if !value.is_object() && !value.is_boolean() {
        return Err(format!("{label} schema must be an object or boolean"));
    }
    Ok(())
}

fn decode<T: for<'de> Deserialize<'de>>(value: Value, label: &str) -> Result<T, String> {
    serde_json::from_value(value)
        .map_err(|error| format!("{label} registration is invalid: {error}"))
}

fn session_scope() -> ExtensionScope {
    ExtensionScope::Session
}

fn text_input() -> Vec<InputModality> {
    vec![InputModality::Text]
}
