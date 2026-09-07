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

#[cfg(test)]
mod coverage_gap {
    use super::*;
    use crate::trigger_engine::types::{
        CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, TriggerAuthority,
        TriggerSource,
    };

    fn sample_trigger(payload: Option<serde_json::Value>) -> Trigger {
        Trigger {
            source: TriggerSource::Mcp {
                server_name: "github".into(),
                method: "notifications/pr.merged".into(),
            },
            source_kind: SourceKind::Mcp,
            source_label: "mcp:github".into(),
            event_label: "pr_merged".into(),
            payload_visibility: PayloadVisibility::Local,
            payload_summary: None,
            payload,
            idempotency_key: "k".into(),
            replacement_policy: ReplacementPolicy::Drop,
            trace_id: "trace-utils".into(),
            authority: TriggerAuthority {
                principal_id: "mcp:github".into(),
                principal_label: "github".into(),
                credential_scope: CredentialScope::User,
                allowed_source_actions: vec![],
                expires_at: None,
            },
            received_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn validated_payload_agent_id_rejects_non_string_and_bad_paths() {
        let trigger = sample_trigger(Some(serde_json::json!({
            "_meta": {"receiver_agent_id": 42},
            "receiver_agent_id": "not-a-uuid",
        })));
        assert_eq!(
            validated_payload_agent_id(&trigger, &["_meta", "receiver_agent_id"]),
            None
        );
        assert_eq!(validated_payload_agent_id(&trigger, &["missing"]), None);
    }

    #[test]
    fn validated_payload_action_class_rejects_invalid_shapes() {
        let trigger = sample_trigger(Some(serde_json::json!({
            "_meta": {"action_class": "Sk-secret"},
            "action_class": "valid.class",
        })));
        assert_eq!(
            validated_payload_action_class(&trigger, &["_meta", "action_class"]),
            None
        );
        assert_eq!(
            validated_payload_action_class(&trigger, &["action_class"]).as_deref(),
            Some("valid.class")
        );

        let empty = sample_trigger(Some(serde_json::json!({"action_class": ""})));
        assert_eq!(
            validated_payload_action_class(&empty, &["action_class"]),
            None
        );
    }

    #[test]
    fn is_valid_action_class_rejects_uppercase_length_and_bad_chars() {
        assert!(!is_valid_action_class("Uppercase"));
        assert!(!is_valid_action_class("a".repeat(65).as_str()));
        assert!(!is_valid_action_class("bad class"));
        assert!(!is_valid_action_class("contains-token-secret"));
        assert!(!is_valid_action_class("sk-prefixed"));
        assert!(is_valid_action_class("valid_action.class:with-all"));
    }

    #[test]
    fn trigger_json_string_handles_missing_paths_and_scalars() {
        let trigger = sample_trigger(Some(serde_json::json!({
            "nested": {"value": 1}
        })));
        assert_eq!(trigger_json_string(&trigger, &["nested"]), None);
        assert_eq!(trigger_json_string(&trigger, &["nested", "value"]), None);
        assert_eq!(trigger_json_string(&trigger, &["nested", "missing"]), None);
    }

    #[test]
    fn cap_control_plane_audit_label_truncates_long_labels() {
        let label = "x".repeat(201);
        let capped = cap_control_plane_audit_label(&label);
        assert_eq!(capped.chars().count(), 200);
        assert!(capped.ends_with('…'));
        assert_eq!(cap_control_plane_audit_label("short"), "short");
    }

    #[test]
    fn cap_trigger_prompt_reason_preserves_short_and_truncates_long() {
        let short = "a short reason";
        assert_eq!(cap_trigger_prompt_reason(short), short);
        let long = "r".repeat(600);
        let capped = cap_trigger_prompt_reason(&long);
        assert_eq!(capped.chars().count(), 512);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn preview_for_banner_preserves_short_and_truncates_long() {
        assert_eq!(preview_for_banner("hello", 10), "hello");
        let capped = preview_for_banner(&"x".repeat(200), 80);
        assert_eq!(capped.chars().count(), 81);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn is_valid_action_class_rejects_bearer_and_control_chars() {
        assert!(!is_valid_action_class("Bearer invalid"));
        assert!(!is_valid_action_class("bad!char"));
        assert!(is_valid_action_class("ok.action_class:with-dashes"));
    }

    #[test]
    fn build_trigger_prompt_request_resolves_nested_and_top_level_fields() {
        let trigger = sample_trigger(Some(serde_json::json!({
            "_meta": {
                "receiver_agent_id": "11111111-1111-4111-8111-111111111111",
                "sender_agent_id": "22222222-2222-4222-8222-222222222222",
                "action_class": "nested.class",
            },
            "receiver_agent_id": "33333333-3333-4333-8333-333333333333",
            "sender_agent_id": "44444444-4444-4444-8444-444444444444",
            "agent_id": "55555555-5555-4555-8555-555555555555",
            "action_class": "top.class",
            "payload_summary": "summary",
        })));
        let request = build_trigger_prompt_request(&trigger, "why".into());
        assert_eq!(
            request.receiver_agent_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            request.sender_agent_id.as_str(),
            "22222222-2222-4222-8222-222222222222"
        );
        assert_eq!(request.action_class, "nested.class");
    }

    #[test]
    fn emit_from_listeners_isolates_panicking_listener() {
        use crate::trigger_engine::event::TriggerEvent;
        let listeners: Arc<Mutex<Vec<TriggerListener>>> = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_sink = calls.clone();
        listeners.lock().push(Arc::new(move |_event: TriggerEvent| {
            panic!("listener panic");
        }));
        listeners.lock().push(Arc::new(move |event: TriggerEvent| {
            let _ = event;
            calls_sink.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        emit_from_listeners(
            &listeners,
            TriggerEvent::TriggerHandlingStart {
                idempotency_key: "k".into(),
                source_kind: SourceKind::Mcp,
                source_label: "src".into(),
                event_label: "evt".into(),
                trace_id: "trace".into(),
            },
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
