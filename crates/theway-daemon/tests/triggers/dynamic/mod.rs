//! Tests for `dynamic` — split out of src (see docs/rust-test-files.md).

use super::*;
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;
use crate::trigger_engine::types::{
    CredentialScope, PayloadVisibility, ReplacementPolicy, SourceKind, TriggerAuthority,
    TriggerSource,
};

#[test]
fn new_trigger_permission_reason_is_value_free() {
    // Provider/Auth gate on PR #139: a tokenized URL or other secret-bearing string
    // smuggled into `condition` / `action` / `spec` must NOT appear in the runtime
    // prompt reason (which lands in audit + UI). The reason names the field shape only;
    // the full bounded args flow through the runtime default `prompt_payload`
    // (`{tool_name, args_keys, args_hash}`) for the embedder card.
    let token_like =
        "https://hub.example/api?token=ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890_super_secret";

    let cases = [
        serde_json::json!({ "condition": token_like, "action": "echo ok" }),
        serde_json::json!({ "condition": "always", "action": token_like }),
        serde_json::json!({ "spec": token_like }),
        serde_json::json!({}),
    ];

    for args in cases {
        let cls = NewTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&args);
        let PermissionClassification::Prompt { reason } = cls else {
            panic!("NewTrigger must always Prompt, got {cls:?} for args {args}");
        };
        assert!(
            !reason.contains("token=ABCDEFGHIJKLMNOPQRSTUVWXYZ"),
            "reason must not echo token-like value substrings; got: {reason}"
        );
        assert!(
            !reason.contains("https://hub.example/api"),
            "reason must not echo URL substrings; got: {reason}"
        );
        assert!(
            !reason.contains("super_secret"),
            "reason must not echo secret substrings; got: {reason}"
        );
    }
}

#[test]
fn parses_chinese_trigger_rule() {
    let spec = concat!(
        "\u{5f53}",
        "\u{5728} github \u{4e0a}\u{6709}\u{65b0} issue",
        "\u{7684}\u{65f6}\u{5019}\u{ff0c}\u{6267}\u{884c} ./notify.sh"
    );
    let parsed = parse_trigger_rule(spec).expect("parse");
    assert_eq!(
        parsed.condition,
        "\u{5728} github \u{4e0a}\u{6709}\u{65b0} issue"
    );
    assert_eq!(parsed.action, "./notify.sh");
}

#[test]
fn parses_english_trigger_rule() {
    let parsed = parse_trigger_rule("when a build finishes, run cargo test").expect("parse");
    assert_eq!(parsed.condition, "a build finishes");
    assert_eq!(parsed.action, "cargo test");
}

#[test]
fn parses_chinese_if_then_trigger_rule() {
    let condition = "\u{73b0}\u{5728}\u{662f} 11pm";
    let action = "\u{5199}\u{4e00}\u{4e2a} tmp \u{6587}\u{4ef6}";
    let spec = format!("\u{5982}\u{679c}{condition}\u{ff0c}\u{5219}{action}");
    let parsed = parse_trigger_rule(&spec).expect("parse");
    assert_eq!(parsed.condition, condition);
    assert_eq!(parsed.action, action);
}

#[test]
fn rejects_missing_action_separator() {
    let err = parse_trigger_rule(&format!("{ZH_WHEN_PREFIX}\u{6709}\u{65b0} issue"))
        .expect_err("missing action");
    assert_eq!(err, ParseTriggerRuleError::MissingAction);
}

#[test]
fn persists_rules_when_storage_path_is_configured() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("triggers.json");
    let registry = DynamicTriggerRegistry::new();
    registry.load_from_path(&path).expect("load empty");
    let rule = registry
        .add_rule("the event says build finished", "echo fired")
        .expect("add");

    let reloaded = DynamicTriggerRegistry::new();
    reloaded.load_from_path(&path).expect("reload");
    assert_eq!(reloaded.list(), vec![rule]);
}

#[test]
fn storage_paths_keep_session_rules_isolated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path_a = dir.path().join("session-a.triggers.json");
    let path_b = dir.path().join("session-b.triggers.json");

    let registry_a = DynamicTriggerRegistry::new();
    registry_a.load_from_path(&path_a).expect("load a");
    registry_a
        .add_rule("event for session a", "echo a")
        .expect("add");

    let registry_b = DynamicTriggerRegistry::new();
    registry_b.load_from_path(&path_b).expect("load b");
    assert!(registry_b.list().is_empty());

    let reloaded_a = DynamicTriggerRegistry::new();
    reloaded_a.load_from_path(&path_a).expect("reload a");
    assert_eq!(reloaded_a.list().len(), 1);
    assert_eq!(reloaded_a.list()[0].condition, "event for session a");
}

#[test]
fn removing_rule_updates_storage_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("triggers.json");
    let registry = DynamicTriggerRegistry::new();
    registry.load_from_path(&path).expect("load empty");
    let rule = registry
        .add_rule("the event says stale", "echo stale")
        .expect("add");

    let removed = registry.remove_rule(&rule.id).expect("remove");
    assert_eq!(removed, Some(rule));

    let reloaded = DynamicTriggerRegistry::new();
    reloaded.load_from_path(&path).expect("reload");
    assert!(reloaded.list().is_empty());
}

#[test]
fn fire_once_rules_can_be_marked_fired() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("event says fire once", "echo once")
        .expect("rule");

    let changed = registry
        .mark_rules_fired(std::slice::from_ref(&rule.id))
        .expect("mark fired");
    assert_eq!(changed.len(), 1);

    let rules = registry.list();
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].enabled);
    assert!(rules[0].fire_once);
    assert!(rules[0].fired_at.is_some());
}

#[test]
fn repeat_rules_are_not_disabled_when_marked_fired() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule_with_options("event says repeat", "echo repeat", false)
        .expect("rule");

    let changed = registry
        .mark_rules_fired(std::slice::from_ref(&rule.id))
        .expect("mark fired");
    assert!(changed.is_empty());

    let rules = registry.list();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].enabled);
    assert!(!rules[0].fire_once);
    assert!(rules[0].fired_at.is_none());
}

#[test]
fn set_rule_enabled_reactivates_fired_fire_once_rule() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_rule("event says reactivate", "echo again")
        .expect("rule");
    registry
        .mark_rules_fired(std::slice::from_ref(&rule.id))
        .expect("mark fired");

    let updated = registry
        .set_rule_enabled(&rule.id, true)
        .expect("enable")
        .expect("rule");
    assert!(updated.enabled);
    assert!(updated.fired_at.is_none());
}

#[test]
fn extracts_dynamic_rule_ids_from_summary() {
    let text =
        "matched dyn-1234567890abcdef1234567890abcdef and dyn-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    assert_eq!(
        extract_dynamic_rule_ids(text),
        vec![
            "dyn-1234567890abcdef1234567890abcdef".to_string(),
            "dyn-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()
        ]
    );
}

#[tokio::test]
async fn periodic_hook_emits_check_trigger_when_rules_exist() {
    let registry = DynamicTriggerRegistry::new();
    registry
        .add_rule("a periodic check arrives", "echo fired")
        .expect("rule");
    let hook = DynamicTriggerCheckHook::with_interval(registry, Duration::from_millis(5));
    let (sink, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let task = tokio::spawn(async move { hook.run(sink).await });
    let trigger = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("hook should emit")
        .expect("trigger");
    task.abort();

    assert_eq!(trigger.source_label, "local:dynamic");
    assert_eq!(trigger.event_label, "dynamic periodic check");
    assert!(
        trigger
            .payload_summary
            .as_deref()
            .unwrap_or_default()
            .contains("1 enabled rule")
    );
}

#[tokio::test]
async fn action_hook_wraps_event_and_rules_for_agent_evaluation() {
    let registry = DynamicTriggerRegistry::new();
    let rule = registry
        .add_from_spec("when the event mentions build finished, run echo done")
        .expect("rule");
    let hook = before_trigger_action_hook(registry);
    let action = hook(
        BeforeTriggerActionContext {
            trigger: Trigger {
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
            },
            runtime: TriggerRuntimeSnapshot {
                dedup_entries: 0,
                active_traces: 0,
                accepted_total: 0,
                deduped_total: 0,
                cycle_suppressed_total: 0,
            },
        },
        CancellationToken::new(),
    )
    .await;

    assert!(action.prompt.contains(&rule.id));
    assert!(action.prompt.contains("build finished"));
    assert!(action.prompt.contains("echo done"));
    assert!(action.prompt.contains("with the available tools"));
    assert!(action.prompt.contains("\"payload\""));
    assert!(action.prompt.contains("environment variables"));
    assert!(
        action
            .prompt
            .contains("include the requested file contents")
    );
    assert!(matches!(action.promote, PromoteAction::None));
}

/// `payload_visibility = Local` (the default for most sources) must drop the raw
/// `payload` JSON from the sub-agent prompt. The dynamic trigger evaluator runs in a
/// sub-agent that calls a model provider, so a payload field populated by any future
/// source (Cloudflare hub, local file-watcher with file contents, etc.) MUST NOT
/// reach the provider context unless the source explicitly declared
/// `PayloadVisibility::Shared`. The previous implementation serialized
/// `trigger.payload` unconditionally, which bypassed the RFC 0 §3.2.2 / RFC 1 §4.2.3
/// privacy contract and was flagged as a HIGH blocker by all reviewers on the
/// `dynamic trigger workflow` commit.
#[tokio::test]
async fn local_payload_visibility_does_not_leak_payload_into_sub_agent_prompt() {
    let registry = DynamicTriggerRegistry::new();
    let _rule = registry
        .add_from_spec("when something happens, run echo nothing")
        .expect("rule");
    let hook = before_trigger_action_hook(registry);
    // Sentinel chosen so a substring search reliably fails if the payload leaks.
    let sentinel = "SECRET_PAYLOAD_SHOULD_NOT_REACH_MODEL_2K7";
    let action = hook(
        BeforeTriggerActionContext {
            trigger: Trigger {
                source: TriggerSource::Local {
                    subkind: "test".into(),
                },
                source_kind: SourceKind::Local,
                source_label: "local:test".into(),
                event_label: "build finished".into(),
                payload_visibility: PayloadVisibility::Local,
                payload_summary: Some("safe summary".into()),
                payload: Some(serde_json::json!({
                    "leaked_field": sentinel,
                    "nested": { "also_leaked": sentinel },
                })),
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
            },
            runtime: TriggerRuntimeSnapshot {
                dedup_entries: 0,
                active_traces: 0,
                accepted_total: 0,
                deduped_total: 0,
                cycle_suppressed_total: 0,
            },
        },
        CancellationToken::new(),
    )
    .await;

    assert!(
        !action.prompt.contains(sentinel),
        "Local payload must not leak into the sub-agent prompt — found sentinel in:\n{}",
        action.prompt
    );
    // The safe `payload_summary` field MUST still survive — we are dropping the raw
    // payload, not the entire envelope.
    assert!(
        action.prompt.contains("safe summary"),
        "payload_summary should still be visible: {}",
        action.prompt
    );
}

/// Counterpart: when a source explicitly opts in to `PayloadVisibility::Shared`, the
/// full payload reaches the sub-agent prompt as before. Pins that the gate is a
/// per-source decision, not a blanket redaction.
#[tokio::test]
async fn shared_payload_visibility_includes_payload_in_sub_agent_prompt() {
    let registry = DynamicTriggerRegistry::new();
    let _rule = registry
        .add_from_spec("when something happens, run echo nothing")
        .expect("rule");
    let hook = before_trigger_action_hook(registry);
    let marker = "shared-payload-marker-must-appear";
    let action = hook(
        BeforeTriggerActionContext {
            trigger: Trigger {
                source: TriggerSource::Mcp {
                    server_name: "test".into(),
                    method: "notification".into(),
                },
                source_kind: SourceKind::Mcp,
                source_label: "mcp:test".into(),
                event_label: "explicit shared".into(),
                payload_visibility: PayloadVisibility::Shared,
                payload_summary: Some("shared event".into()),
                payload: Some(serde_json::json!({ "value": marker })),
                idempotency_key: "shared-key".into(),
                replacement_policy: ReplacementPolicy::Drop,
                trace_id: "trace-shared".into(),
                authority: TriggerAuthority {
                    principal_id: "mcp:test".into(),
                    principal_label: "mcp:test".into(),
                    credential_scope: CredentialScope::User,
                    allowed_source_actions: vec![],
                    expires_at: None,
                },
                received_at: Utc::now(),
            },
            runtime: TriggerRuntimeSnapshot {
                dedup_entries: 0,
                active_traces: 0,
                accepted_total: 0,
                deduped_total: 0,
                cycle_suppressed_total: 0,
            },
        },
        CancellationToken::new(),
    )
    .await;

    assert!(
        action.prompt.contains(marker),
        "Shared payload should reach the sub-agent prompt — marker missing from:\n{}",
        action.prompt
    );
}

/// `Redacted` visibility behaves like `Local` for prompt rendering: the raw payload is
/// dropped even though the source may have attached one. The runtime contract says
/// `Redacted` is the strongest of the three.
#[tokio::test]
async fn redacted_payload_visibility_does_not_leak_payload_into_sub_agent_prompt() {
    let registry = DynamicTriggerRegistry::new();
    let _rule = registry
        .add_from_spec("when something happens, run echo nothing")
        .expect("rule");
    let hook = before_trigger_action_hook(registry);
    let sentinel = "REDACTED_FIELD_MUST_BE_DROPPED_9X4";
    let action = hook(
        BeforeTriggerActionContext {
            trigger: Trigger {
                source: TriggerSource::Local {
                    subkind: "test".into(),
                },
                source_kind: SourceKind::Local,
                source_label: "local:test".into(),
                event_label: "sensitive event".into(),
                payload_visibility: PayloadVisibility::Redacted,
                payload_summary: Some("redacted summary only".into()),
                payload: Some(serde_json::json!({ "credential": sentinel })),
                idempotency_key: "redacted-key".into(),
                replacement_policy: ReplacementPolicy::Drop,
                trace_id: "trace-redacted".into(),
                authority: TriggerAuthority {
                    principal_id: "test".into(),
                    principal_label: "test".into(),
                    credential_scope: CredentialScope::User,
                    allowed_source_actions: vec![],
                    expires_at: None,
                },
                received_at: Utc::now(),
            },
            runtime: TriggerRuntimeSnapshot {
                dedup_entries: 0,
                active_traces: 0,
                accepted_total: 0,
                deduped_total: 0,
                cycle_suppressed_total: 0,
            },
        },
        CancellationToken::new(),
    )
    .await;

    assert!(
        !action.prompt.contains(sentinel),
        "Redacted payload must not leak into the sub-agent prompt — found sentinel in:\n{}",
        action.prompt
    );
}
