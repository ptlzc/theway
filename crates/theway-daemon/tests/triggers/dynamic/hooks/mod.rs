//! Tests for `triggers::dynamic::hooks` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::collections::HashSet;

use chrono::Utc;
use serde_json::Value;

use crate::trigger_engine::event::{TriggerEvent, TriggerListener};
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, TriggerAuthority,
    TriggerSource,
};

fn runtime_snapshot() -> TriggerRuntimeSnapshot {
    TriggerRuntimeSnapshot {
        dedup_entries: 0,
        active_traces: 0,
        accepted_total: 0,
        deduped_total: 0,
        cycle_suppressed_total: 0,
    }
}

fn local_trigger() -> Trigger {
    Trigger {
        source: TriggerSource::Local {
            subkind: "test".into(),
        },
        source_kind: SourceKind::Local,
        source_label: "local:test".into(),
        event_label: "build finished".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: Some("build finished successfully".into()),
        payload: None,
        idempotency_key: "test-key".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-test".into(),
        authority: TriggerAuthority {
            principal_id: "test".into(),
            principal_label: "test".into(),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: Utc::now(),
    }
}

fn mcp_trigger(server_name: &str, summary: Option<&str>) -> Trigger {
    Trigger {
        source: TriggerSource::Mcp {
            server_name: server_name.into(),
            method: "notification".into(),
        },
        source_kind: SourceKind::Mcp,
        source_label: format!("mcp:{server_name}"),
        event_label: "push".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: summary.map(str::to_string),
        payload: None,
        idempotency_key: format!("mcp:{server_name}:1"),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: format!("trace-{server_name}"),
        authority: TriggerAuthority {
            principal_id: format!("mcp:{server_name}"),
            principal_label: format!("mcp:{server_name}"),
            credential_scope: CredentialScope::User,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: Utc::now(),
    }
}

fn context(trigger: Trigger) -> BeforeTriggerActionContext {
    BeforeTriggerActionContext {
        trigger,
        runtime: runtime_snapshot(),
    }
}

fn inner_hook() -> BeforeTriggerActionHook {
    Arc::new(|_ctx: BeforeTriggerActionContext, _cancel: CancellationToken| {
        Box::pin(async move {
            TriggerAction {
                prompt: "inner-hook".into(),
                promote: PromoteAction::None,
                promote_requires_approval: false,
                delivery: TriggerDelivery::SubAgent,
            }
        })
    })
}

#[test]
fn check_hook_metadata_and_default_status() {
    let registry = DynamicTriggerRegistry::new();
    let hook = DynamicTriggerCheckHook::new(registry);

    assert_eq!(hook.label(), "local:dynamic");
    let status = hook.status();
    assert_eq!(
        status.subscription_labels,
        vec!["dynamic trigger periodic check".to_string()]
    );
    assert!(matches!(
        status.state,
        HookState::Disconnected { .. }
    ));
}

#[test]
fn build_trigger_emits_local_dynamic_envelope_with_summary() {
    let registry = DynamicTriggerRegistry::new();
    let hook = DynamicTriggerCheckHook::with_interval(registry, Duration::from_secs(60));

    let trigger = hook.build_trigger(3);

    assert_eq!(trigger.source_label, "local:dynamic");
    assert_eq!(trigger.event_label, "dynamic periodic check");
    assert_eq!(trigger.source_kind, SourceKind::Local);
    assert!(matches!(
        trigger.source,
        TriggerSource::Local { ref subkind } if subkind == "dynamic"
    ));
    assert_eq!(trigger.payload_visibility, PayloadVisibility::Local);
    assert_eq!(trigger.payload, None);
    assert!(
        trigger
            .payload_summary
            .as_deref()
            .unwrap_or_default()
            .contains("3 enabled rule(s)"),
        "{}",
        trigger.payload_summary.as_deref().unwrap_or_default()
    );
    assert!(trigger.idempotency_key.starts_with("local:dynamic:"));
    assert_eq!(trigger.replacement_policy, ReplacementPolicy::Drop);
    assert_eq!(trigger.authority.principal_id, "local:dynamic");
}

#[tokio::test]
async fn direct_inject_run_server_injects_summary_and_runs_parent_turn() {
    let inject_summary = HashSet::new();
    let inject_and_run: HashSet<String> = ["gh".to_string(), "both".to_string()].into_iter().collect();
    let hook = direct_inject_action_hook(inject_summary, inject_and_run, inner_hook());

    // Server is only in inject_and_run, payload_summary present: verbatim prompt.
    let action = hook(context(mcp_trigger("gh", Some("pushed pr #9"))), CancellationToken::new()).await;
    assert_eq!(action.prompt, "pushed pr #9");
    assert_eq!(action.delivery, TriggerDelivery::InjectAndRun);
    assert!(matches!(action.promote, PromoteAction::None));

    // Server in both sets: inject_and_run still wins.
    let action = hook(context(mcp_trigger("both", Some("both sets"))), CancellationToken::new()).await;
    assert_eq!(action.delivery, TriggerDelivery::InjectAndRun);
    assert_eq!(action.prompt, "both sets");

    // No payload_summary: fallback to source_label/event_label.
    let action = hook(context(mcp_trigger("gh", None)), CancellationToken::new()).await;
    assert_eq!(action.delivery, TriggerDelivery::InjectAndRun);
    assert_eq!(action.prompt, "mcp:gh fired: push");
}

#[tokio::test]
async fn direct_inject_summary_server_injects_verbatim_summary_or_noop() {
    let inject_summary: HashSet<String> = ["gh".to_string()].into_iter().collect();
    let inject_and_run = HashSet::new();
    let hook = direct_inject_action_hook(inject_summary, inject_and_run, inner_hook());

    let action = hook(context(mcp_trigger("gh", Some("raw summary"))), CancellationToken::new()).await;
    assert_eq!(action.delivery, TriggerDelivery::InjectSummary);
    assert_eq!(action.prompt, String::new());
    assert!(matches!(
        action.promote,
        PromoteAction::PromoteSummaryNow {
            template_body: Some(ref body)
        } if body == "{{trigger.payload_summary}}"
    ));

    let action = hook(context(mcp_trigger("gh", None)), CancellationToken::new()).await;
    assert_eq!(action.delivery, TriggerDelivery::InjectSummary);
    assert!(matches!(action.promote, PromoteAction::None));
}

#[tokio::test]
async fn direct_inject_hook_falls_through_to_inner_for_other_sources() {
    let inject_summary: HashSet<String> = ["gh".to_string()].into_iter().collect();
    let inject_and_run: HashSet<String> = ["gh".to_string()].into_iter().collect();
    let hook = direct_inject_action_hook(inject_summary, inject_and_run, inner_hook());

    let action = hook(context(local_trigger()), CancellationToken::new()).await;

    assert_eq!(action.prompt, "inner-hook");
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
}

#[test]
fn fire_once_listener_marks_matching_fired_rules() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("a periodic check arrives", "echo fired")
        .expect("rule");
    assert!(rule.fire_once);

    let listener = fire_once_trigger_listener(registry.clone());
    listener(TriggerEvent::TriggerCompleted {
        trace_id: "trace-1".into(),
        summary: Some(format!("matched {} today", rule.id)),
        cost_usd: None,
        details: Value::Null,
    });

    let rules = registry.list();
    assert!(!rules[0].enabled, "fire-once rule should be disabled");
    assert!(rules[0].fired_at.is_some());
}

#[test]
fn fire_once_listener_ignores_non_matching_or_non_terminal_events() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("a periodic check arrives", "echo fired")
        .expect("rule");

    let listener = fire_once_trigger_listener(registry.clone());
    // Summary without a valid `dyn-` id.
    listener(TriggerEvent::TriggerCompleted {
        trace_id: "trace-1".into(),
        summary: Some("no dynamic ids here".into()),
        cost_usd: None,
        details: Value::Null,
    });
    // Other event variants are ignored.
    listener(TriggerEvent::TriggerFailed {
        trace_id: "trace-1".into(),
        reason: "aborted".into(),
    });

    let rules = registry.list();
    assert!(rules[0].enabled, "unrelated events must not disable the rule");
    assert_eq!(rules[0].id, rule.id);
}

#[tokio::test]
async fn action_hook_with_no_enabled_rules_returns_default_action() {
    let registry = DynamicTriggerRegistry::new();
    let hook = before_trigger_action_hook(registry);

    let action = hook(context(local_trigger()), CancellationToken::new()).await;

    assert_eq!(action.prompt, "local:test fired: build finished");
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
    assert!(matches!(action.promote, PromoteAction::None));
}

#[tokio::test]
async fn action_hook_promotes_to_chat_when_rule_requests_it() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule_with_flags("a periodic check arrives", "echo fired", true, true)
        .expect("rule");
    let hook = before_trigger_action_hook(registry);

    let action = hook(context(local_trigger()), CancellationToken::new()).await;

    assert!(action.prompt.contains(&rule.id));
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
    assert!(!action.promote_requires_approval);
    match action.promote {
        PromoteAction::PromoteSummaryWhenResultDetailsMatch {
            condition: PromotionCondition::AnyOf { any_of, .. },
            ..
        } => {
            assert_eq!(any_of, vec![rule.id]);
        }
        other => panic!("expected result-details promotion, got {other:?}"),
    }
}

#[test]
fn render_dynamic_trigger_prompt_includes_envelope_and_rules() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("a periodic check arrives", "echo fired")
        .expect("rule");

    let prompt = render_dynamic_trigger_prompt(&local_trigger(), std::slice::from_ref(&rule));

    assert!(prompt.contains("A trigger check event arrived."), "{prompt}");
    assert!(prompt.contains("Dynamic trigger rules:"), "{prompt}");
    assert!(prompt.contains(&rule.id), "{prompt}");
    assert!(prompt.contains("echo fired"), "{prompt}");
    assert!(prompt.contains("\"payload\": null"), "{prompt}");
    assert!(prompt.contains("\"source_label\": \"local:test\""), "{prompt}");
}

#[test]
fn fire_once_listener_type_is_shared_listener() {
    let registry = DynamicTriggerRegistry::new();
    let listener = fire_once_trigger_listener(registry);
    let _: TriggerListener = listener;
}
