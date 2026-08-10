//! Small shared helpers for the execution pipeline: audit-label / reason caps,
//! prompt-request construction with payload validation, listener fan-out and
//! banner preview truncation.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::types::Trigger;

use super::promotion::{PROMOTION_BODY_CAP_BYTES, sha256_hex, truncate_on_char_boundary};
use super::types::TriggerPromptRequest;

const CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS: usize = 200;

pub(super) fn cap_control_plane_audit_label(label: &str) -> String {
    if label.chars().count() <= CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS {
        return label.to_string();
    }
    let mut out: String = label
        .chars()
        .take(CONTROL_PLANE_PROMPT_LABEL_CAP_CHARS.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

pub(super) fn build_trigger_prompt_request(
    trigger: &Trigger,
    reason: String,
) -> TriggerPromptRequest {
    let receiver_agent_id = validated_payload_agent_id(trigger, &["_meta", "receiver_agent_id"])
        .or_else(|| validated_payload_agent_id(trigger, &["receiver_agent_id"]));
    let sender_agent_id = validated_payload_agent_id(trigger, &["_meta", "sender_agent_id"])
        .or_else(|| validated_payload_agent_id(trigger, &["sender_agent_id"]))
        .or_else(|| validated_payload_agent_id(trigger, &["agent_id"]))
        .unwrap_or_else(|| cap_control_plane_audit_label(&trigger.authority.principal_id));
    let action_class = validated_payload_action_class(trigger, &["_meta", "action_class"])
        .or_else(|| validated_payload_action_class(trigger, &["action_class"]))
        .unwrap_or_else(|| cap_control_plane_audit_label(&trigger.event_label));
    let trigger_summary = trigger
        .payload_summary
        .clone()
        .map(|summary| truncate_on_char_boundary(summary, PROMOTION_BODY_CAP_BYTES).0);
    let payload = serde_json::json!({
        "source_kind": trigger.source_kind,
        "source_label": cap_control_plane_audit_label(&trigger.source_label),
        "event_label": cap_control_plane_audit_label(&trigger.event_label),
        "payload_visibility": trigger.payload_visibility,
        "payload_summary": trigger_summary,
        "authority": {
            "principal_id": trigger.authority.principal_id.clone(),
            "principal_label": cap_control_plane_audit_label(&trigger.authority.principal_label),
            "credential_scope": trigger.authority.credential_scope,
            "allowed_source_actions": trigger.authority.allowed_source_actions.clone(),
        }
    });
    let binding = serde_json::json!([
        "trigger_prompt:v1",
        trigger.idempotency_key.clone(),
        trigger.trace_id.clone(),
        trigger.source_kind,
        trigger.source_label.clone(),
        trigger.event_label.clone(),
        receiver_agent_id.clone(),
        sender_agent_id.clone(),
        action_class.clone(),
    ]);
    let trigger_prompt_id = sha256_hex(&binding.to_string());
    TriggerPromptRequest {
        trigger_prompt_id,
        trace_id: trigger.trace_id.clone(),
        source_label: cap_control_plane_audit_label(&trigger.source_label),
        receiver_agent_id,
        sender_agent_id,
        action_class,
        trigger_summary,
        payload,
        reason: cap_trigger_prompt_reason(&reason),
    }
}

fn validated_payload_agent_id(trigger: &Trigger, path: &[&str]) -> Option<String> {
    let value = trigger_json_string(trigger, path)?;
    uuid::Uuid::parse_str(&value).ok()?;
    Some(value)
}

fn validated_payload_action_class(trigger: &Trigger, path: &[&str]) -> Option<String> {
    let value = trigger_json_string(trigger, path)?;
    is_valid_action_class(&value).then_some(value)
}

fn trigger_json_string(trigger: &Trigger, path: &[&str]) -> Option<String> {
    let mut value = trigger.payload.as_ref()?;
    for key in path {
        value = value.get(*key)?;
    }
    value.as_str().map(str::to_string)
}

fn is_valid_action_class(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("sk-") || lower.contains("bearer") || lower.contains("token") {
        return false;
    }
    value.len() <= 64
        && first.is_ascii_lowercase()
        && chars.all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-' | '.' | ':')
        })
}

const TRIGGER_PROMPT_REASON_CAP_CHARS: usize = 512;

pub(super) fn cap_trigger_prompt_reason(reason: &str) -> String {
    if reason.chars().count() <= TRIGGER_PROMPT_REASON_CAP_CHARS {
        return reason.to_string();
    }
    let mut out: String = reason
        .chars()
        .take(TRIGGER_PROMPT_REASON_CAP_CHARS.saturating_sub(1))
        .collect();
    out.push('…');
    out
}

/// Emit a [`TriggerEvent`] to a snapshot of the listener registry, isolating each listener
/// with `catch_unwind` so a single panicking listener cannot poison the others. Mirrors
/// the contract of `TriggerExecutor::emit` but operates on a cloned `Arc` of listeners (so
/// the spawned sub-agent task does not need a `TriggerExecutor` reference).
pub(super) fn emit_from_listeners(
    listeners: &Arc<Mutex<Vec<TriggerListener>>>,
    event: TriggerEvent,
) {
    let snapshot: Vec<TriggerListener> = listeners.lock().clone();
    for listener in snapshot {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(event.clone())));
    }
}

/// Bounded preview text for status banners. Avoids panicking on multi-byte char boundaries
/// by walking char count, not byte count.
pub(super) fn preview_for_banner(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}
