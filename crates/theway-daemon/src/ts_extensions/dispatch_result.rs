use serde_json::Value;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionAction, ExtensionActionBatch, ExtensionActionKind,
    ExtensionGateDecision, ExtensionHookClass, ExtensionLifecycleEvent,
};

pub(super) fn empty_batch() -> ExtensionActionBatch {
    ExtensionActionBatch {
        abi_major: ExtensionAbiMajor::V2,
        decision: None,
        actions: Vec::new(),
    }
}

pub(super) fn decode_batch(value: Value) -> Result<ExtensionActionBatch, String> {
    if value.is_null() {
        return Ok(empty_batch());
    }
    serde_json::from_value(value)
        .map_err(|error| format!("extension returned an invalid action batch: {error}"))
}

pub(super) fn failed_gate_decision() -> ExtensionGateDecision {
    ExtensionGateDecision::Deny {
        code: "extension_gate_failed".into(),
        message: "required extension gate failed".into(),
    }
}

pub(super) fn accept_transform_batch(
    event: ExtensionLifecycleEvent,
    current_payload: &mut Value,
    aggregate: &mut ExtensionActionBatch,
    next: ExtensionActionBatch,
) -> Result<bool, String> {
    let mut candidate = current_payload.clone();
    let mut primary = None;
    let mut remaining = Vec::new();
    for action in next.actions {
        if is_primary_transform(event, action.kind) {
            primary = Some(apply_primary_transform(event, &mut candidate, action)?);
        } else {
            remaining.push(action);
        }
    }
    *current_payload = candidate;
    aggregate.actions.extend(remaining);
    if let Some(primary) = primary {
        aggregate
            .actions
            .retain(|current| current.kind != primary.kind);
        aggregate.actions.push(primary);
    }
    Ok(false)
}

fn apply_primary_transform(
    event: ExtensionLifecycleEvent,
    current: &mut Value,
    action: ExtensionAction,
) -> Result<ExtensionAction, String> {
    match event {
        ExtensionLifecycleEvent::Input => {
            replace_object_field(current, &action.payload, "message", Value::is_object)?;
            Ok(action_with_field(action.kind, current, "message"))
        }
        ExtensionLifecycleEvent::BeforeRun => {
            let patch = action
                .payload
                .as_object()
                .ok_or_else(|| "before_run patch must be an object".to_string())?;
            if patch
                .keys()
                .any(|key| key != "systemPrompt" && key != "messages")
            {
                return Err("before_run patch contains an unknown field".into());
            }
            if patch
                .get("systemPrompt")
                .is_some_and(|value| !value.is_null() && !value.is_string())
                || patch.get("messages").is_some_and(|value| !value.is_array())
            {
                return Err("before_run patch has an invalid field type".into());
            }
            let current = current
                .as_object_mut()
                .ok_or_else(|| "before_run waterfall value must be an object".to_string())?;
            if let Some(prompt) = patch.get("systemPrompt") {
                current.insert("systemPrompt".into(), prompt.clone());
            }
            if let Some(messages) = patch.get("messages").and_then(Value::as_array) {
                current
                    .entry("messages")
                    .or_insert_with(|| Value::Array(Vec::new()))
                    .as_array_mut()
                    .ok_or_else(|| "before_run messages waterfall value is invalid".to_string())?
                    .extend(messages.iter().cloned());
            }
            Ok(ExtensionAction {
                kind: action.kind,
                payload: Value::Object(current.clone()),
            })
        }
        ExtensionLifecycleEvent::Context => {
            replace_object_field(current, &action.payload, "messages", Value::is_array)?;
            Ok(action_with_field(action.kind, current, "messages"))
        }
        ExtensionLifecycleEvent::BeforeModelRequest
        | ExtensionLifecycleEvent::BeforeProviderRequestHeaders
        | ExtensionLifecycleEvent::BeforeProviderRequestRaw => {
            replace_object_field(current, &action.payload, "request", Value::is_object)?;
            Ok(action_with_field(action.kind, current, "request"))
        }
        ExtensionLifecycleEvent::MessageEnd => {
            replace_object_field(current, &action.payload, "message", Value::is_object)?;
            Ok(action_with_field(action.kind, current, "message"))
        }
        ExtensionLifecycleEvent::ToolResult => {
            replace_object_field(current, &action.payload, "result", Value::is_object)?;
            if let Some(is_error) = action.payload.get("isError") {
                if !is_error.is_boolean() && !is_error.is_null() {
                    return Err("tool result replacement isError must be a boolean".into());
                }
                current
                    .as_object_mut()
                    .expect("runtime payload is an object")
                    .insert("isError".into(), is_error.clone());
            }
            let mut payload = serde_json::Map::new();
            payload.insert("result".into(), current["result"].clone());
            if let Some(is_error) = current.get("isError") {
                payload.insert("isError".into(), is_error.clone());
            }
            Ok(ExtensionAction {
                kind: action.kind,
                payload: Value::Object(payload),
            })
        }
        _ => Err("event does not admit a primary transform action".into()),
    }
}

fn replace_object_field(
    current: &mut Value,
    action_payload: &Value,
    field: &str,
    accepts: fn(&Value) -> bool,
) -> Result<(), String> {
    let replacement = action_payload
        .get(field)
        .filter(|value| accepts(value))
        .cloned()
        .ok_or_else(|| format!("transform action requires a valid {field} field"))?;
    current
        .as_object_mut()
        .ok_or_else(|| "transform waterfall value must be an object".to_string())?
        .insert(field.into(), replacement);
    Ok(())
}

fn action_with_field(kind: ExtensionActionKind, current: &Value, field: &str) -> ExtensionAction {
    let mut payload = serde_json::Map::new();
    payload.insert(field.into(), current[field].clone());
    ExtensionAction {
        kind,
        payload: Value::Object(payload),
    }
}

pub(super) fn merge_batch(
    event: ExtensionLifecycleEvent,
    class: ExtensionHookClass,
    aggregate: &mut ExtensionActionBatch,
    next: ExtensionActionBatch,
) -> bool {
    if class == ExtensionHookClass::Transform {
        for action in next.actions {
            if is_primary_transform(event, action.kind) {
                aggregate
                    .actions
                    .retain(|current| current.kind != action.kind);
            }
            aggregate.actions.push(action);
        }
    } else {
        aggregate.actions.extend(next.actions);
    }
    if let Some(decision) = next.decision {
        let stop = matches!(
            decision,
            ExtensionGateDecision::Deny { .. } | ExtensionGateDecision::Cancel { .. }
        );
        aggregate.decision = Some(decision);
        stop
    } else {
        false
    }
}

fn is_primary_transform(event: ExtensionLifecycleEvent, kind: ExtensionActionKind) -> bool {
    matches!(
        (event, kind),
        (
            ExtensionLifecycleEvent::Input,
            ExtensionActionKind::ReplaceInput
        ) | (
            ExtensionLifecycleEvent::BeforeRun,
            ExtensionActionKind::PatchRunContext
        ) | (
            ExtensionLifecycleEvent::Context,
            ExtensionActionKind::ReplaceContext
        ) | (
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionActionKind::ReplaceModelRequest
        ) | (
            ExtensionLifecycleEvent::BeforeProviderRequestHeaders,
            ExtensionActionKind::ReplaceProviderHeaders
        ) | (
            ExtensionLifecycleEvent::BeforeProviderRequestRaw,
            ExtensionActionKind::ReplaceProviderPayload
        ) | (
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionActionKind::ReplaceMessage
        ) | (
            ExtensionLifecycleEvent::ToolResult,
            ExtensionActionKind::ReplaceToolResult
        )
    )
}
