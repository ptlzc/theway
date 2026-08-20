use std::collections::BTreeSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionActionKind, ExtensionCancellationContext, ExtensionDeliveryPolicy,
    ExtensionEventContext, ExtensionEventEnvelope, ExtensionHookClass, ExtensionHookContract,
    ExtensionHookDeadline, ExtensionHookFailurePolicy, ExtensionLifecycleEvent, ExtensionScopeIds,
};
use theway_core::agent::runtime_extensions::RuntimeExtensionInvocation;

const MIN_HOOK_PRIORITY: i32 = -1_000_000;
const MAX_HOOK_PRIORITY: i32 = 1_000_000;

/// Host-owned execution budgets. These values are operational policy and are
/// intentionally not part of the stable extension ABI.
#[derive(Clone, Debug)]
pub struct RuntimeExtensionHostConfig {
    pub fast_deadline: Duration,
    pub standard_deadline: Duration,
    pub long_deadline: Duration,
    pub max_actions: usize,
    pub broker_operation_quota: usize,
    pub observation_queue_capacity: usize,
    pub circuit_failure_threshold: usize,
}

impl Default for RuntimeExtensionHostConfig {
    fn default() -> Self {
        Self {
            fast_deadline: Duration::from_millis(100),
            standard_deadline: Duration::from_millis(500),
            long_deadline: Duration::from_secs(2),
            max_actions: 64,
            broker_operation_quota: 32,
            observation_queue_capacity: 1,
            circuit_failure_threshold: 3,
        }
    }
}

impl RuntimeExtensionHostConfig {
    pub(super) fn deadline(&self, class: ExtensionHookDeadline) -> Duration {
        match class {
            ExtensionHookDeadline::Fast => self.fast_deadline,
            ExtensionHookDeadline::Standard => self.standard_deadline,
            ExtensionHookDeadline::Long => self.long_deadline,
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.fast_deadline.is_zero()
            || self.standard_deadline.is_zero()
            || self.long_deadline.is_zero()
        {
            return Err("extension hook deadlines must be greater than zero".into());
        }
        if self.max_actions == 0
            || self.broker_operation_quota == 0
            || self.observation_queue_capacity == 0
            || self.circuit_failure_threshold == 0
        {
            return Err("extension execution limits must be greater than zero".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(super) struct HookRegistration {
    pub registration_id: u64,
    pub event: ExtensionLifecycleEvent,
    pub class: ExtensionHookClass,
    pub payload_schema: Value,
    pub priority: i32,
    pub deadline: ExtensionHookDeadline,
    pub delivery: ExtensionDeliveryPolicy,
    pub failure: ExtensionHookFailurePolicy,
    pub sequence: u64,
    pub contract: ExtensionHookContract,
}

impl HookRegistration {
    pub(super) fn accepts_payload(&self, payload: &Value) -> bool {
        matches_schema(&self.payload_schema, payload)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRegistration {
    registration_id: u64,
    event: ExtensionLifecycleEvent,
    #[serde(default)]
    descriptor: RawDescriptor,
    sequence: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawDescriptor {
    #[serde(default)]
    class: Option<ExtensionHookClass>,
    #[serde(default)]
    payload_schema: Option<Value>,
    #[serde(default)]
    allowed_actions: Option<Vec<ExtensionActionKind>>,
    #[serde(default)]
    priority: Option<i32>,
    #[serde(default)]
    deadline: Option<ExtensionHookDeadline>,
    #[serde(default)]
    delivery: Option<ExtensionDeliveryPolicy>,
    #[serde(default)]
    failure: Option<ExtensionHookFailurePolicy>,
}

pub(super) fn validate_registrations(metadata: Value) -> Result<Vec<HookRegistration>, String> {
    let registrations = metadata
        .get("registrations")
        .cloned()
        .ok_or_else(|| "extension setup did not return registrations".to_string())?;
    let raw: Vec<RawRegistration> = serde_json::from_value(registrations)
        .map_err(|error| format!("extension registrations are invalid: {error}"))?;
    let mut ids = BTreeSet::new();
    let mut validated = Vec::with_capacity(raw.len());
    for registration in raw {
        if !ids.insert(registration.registration_id) {
            return Err("extension registration ids must be unique".into());
        }
        let class = registration
            .descriptor
            .class
            .unwrap_or_else(|| default_class(registration.event));
        let contract = ExtensionHookContract::for_hook(registration.event, class)
            .map_err(|error| error.message)?;
        if let Some(actions) = &registration.descriptor.allowed_actions {
            let declared: BTreeSet<_> = actions.iter().copied().collect();
            let canonical: BTreeSet<_> = contract.allowed_actions.iter().copied().collect();
            if declared != canonical || declared.len() != actions.len() {
                return Err(format!(
                    "hook {:?}/{class:?} allowedActions must match the ABI contract",
                    registration.event
                ));
            }
        }
        if registration
            .descriptor
            .deadline
            .is_some_and(|deadline| deadline != contract.deadline)
        {
            return Err(format!(
                "hook {:?}/{class:?} deadline must match the ABI contract",
                registration.event
            ));
        }
        if registration
            .descriptor
            .delivery
            .is_some_and(|delivery| delivery != contract.delivery)
        {
            return Err(format!(
                "hook {:?}/{class:?} delivery must match the ABI contract",
                registration.event
            ));
        }
        if registration
            .descriptor
            .failure
            .is_some_and(|failure| failure != contract.failure)
        {
            return Err(format!(
                "hook {:?}/{class:?} failure must match the ABI contract",
                registration.event
            ));
        }
        let priority = registration.descriptor.priority.unwrap_or_default();
        if !(MIN_HOOK_PRIORITY..=MAX_HOOK_PRIORITY).contains(&priority) {
            return Err(format!(
                "hook {:?} priority must be between {MIN_HOOK_PRIORITY} and {MAX_HOOK_PRIORITY}",
                registration.event
            ));
        }
        let payload_schema = registration
            .descriptor
            .payload_schema
            .unwrap_or_else(|| serde_json::json!({"type": "object"}));
        validate_schema(&payload_schema)?;
        validated.push(HookRegistration {
            registration_id: registration.registration_id,
            event: registration.event,
            class,
            payload_schema,
            priority,
            deadline: contract.deadline,
            delivery: contract.delivery,
            failure: contract.failure,
            sequence: registration.sequence,
            contract,
        });
    }
    validated.sort_by(|left, right| {
        left.event
            .cmp(&right.event)
            .then_with(|| left.class.cmp(&right.class))
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.sequence.cmp(&right.sequence))
    });
    Ok(validated)
}

fn default_class(event: ExtensionLifecycleEvent) -> ExtensionHookClass {
    if matches!(
        event,
        ExtensionLifecycleEvent::BeforeSessionSwitch
            | ExtensionLifecycleEvent::BeforeSessionFork
            | ExtensionLifecycleEvent::BeforeModelSelection
            | ExtensionLifecycleEvent::ToolCall
            | ExtensionLifecycleEvent::BeforeCompaction
    ) {
        ExtensionHookClass::Gate
    } else if matches!(
        event,
        ExtensionLifecycleEvent::Input
            | ExtensionLifecycleEvent::BeforeRun
            | ExtensionLifecycleEvent::Context
            | ExtensionLifecycleEvent::BeforeModelRequest
            | ExtensionLifecycleEvent::BeforeProviderRequestHeaders
            | ExtensionLifecycleEvent::BeforeProviderRequestRaw
            | ExtensionLifecycleEvent::MessageEnd
            | ExtensionLifecycleEvent::ToolResult
    ) {
        ExtensionHookClass::Transform
    } else {
        ExtensionHookClass::Observe
    }
}

fn validate_schema(schema: &Value) -> Result<(), String> {
    let Value::Object(object) = schema else {
        return if schema.is_boolean() {
            Ok(())
        } else {
            Err("hook payloadSchema must be a JSON Schema object or boolean".into())
        };
    };
    if let Some(kind) = object.get("type") {
        let valid = kind.as_str().is_some_and(|kind| {
            matches!(
                kind,
                "null" | "boolean" | "number" | "integer" | "string" | "array" | "object"
            )
        });
        if !valid {
            return Err("hook payloadSchema type is invalid".into());
        }
    }
    if object.get("required").is_some_and(|required| {
        required
            .as_array()
            .is_none_or(|items| items.iter().any(|item| !item.is_string()))
    }) {
        return Err("hook payloadSchema required must be an array of strings".into());
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err("hook payloadSchema properties must be an object".into());
        };
        for property in properties.values() {
            validate_schema(property)?;
        }
    }
    if let Some(items) = object.get("items") {
        validate_schema(items)?;
    }
    Ok(())
}

fn matches_schema(schema: &Value, value: &Value) -> bool {
    if let Some(allowed) = schema.as_bool() {
        return allowed;
    }
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let matches = match kind {
            "null" => value.is_null(),
            "boolean" => value.is_boolean(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "string" => value.is_string(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => false,
        };
        if !matches {
            return false;
        }
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let Some(object) = value.as_object() else {
            return false;
        };
        if required
            .iter()
            .filter_map(Value::as_str)
            .any(|key| !object.contains_key(key))
        {
            return false;
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        if properties.iter().any(|(key, schema)| {
            object
                .get(key)
                .is_some_and(|value| !matches_schema(schema, value))
        }) {
            return false;
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false))
            && object.keys().any(|key| !properties.contains_key(key))
        {
            return false;
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array())
        && values.iter().any(|value| !matches_schema(items, value))
    {
        return false;
    }
    true
}

pub(super) fn envelope(
    extension_id: &str,
    session_id: &str,
    cwd: &str,
    sequence: u64,
    event: ExtensionLifecycleEvent,
    payload: Value,
) -> ExtensionEventEnvelope {
    ExtensionEventEnvelope {
        abi_major: ExtensionAbiMajor::V2,
        event,
        context: ExtensionEventContext {
            extension_id: extension_id.to_string(),
            session_id: session_id.to_string(),
            cwd: cwd.to_string(),
            sequence,
            scope: ExtensionScopeIds::default(),
            model: None,
            has_interactive_client: false,
            cancellation: ExtensionCancellationContext::default(),
        },
        payload,
    }
}

pub(super) fn runtime_envelope(
    extension_id: &str,
    invocation: &RuntimeExtensionInvocation,
) -> ExtensionEventEnvelope {
    runtime_envelope_with_payload(extension_id, invocation, invocation.payload().clone())
}

pub(super) fn runtime_envelope_with_payload(
    extension_id: &str,
    invocation: &RuntimeExtensionInvocation,
    payload: Value,
) -> ExtensionEventEnvelope {
    let context = invocation.context();
    ExtensionEventEnvelope {
        abi_major: ExtensionAbiMajor::V2,
        event: invocation.event(),
        context: ExtensionEventContext {
            extension_id: extension_id.to_string(),
            session_id: context.session_id.clone(),
            cwd: context.cwd.clone(),
            sequence: context.sequence,
            scope: context.scope.clone(),
            model: context.model.clone(),
            has_interactive_client: context.has_interactive_client,
            cancellation: ExtensionCancellationContext {
                cancelled: context.cancelled,
                deadline_unix_ms: context.deadline_unix_ms,
            },
        },
        payload,
    }
}
