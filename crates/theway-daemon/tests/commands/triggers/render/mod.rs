//! Tests for `commands::triggers::render` — split out of src (see docs/rust-test-files.md).

use chrono::{DateTime, Utc};
use theway_core::SessionTreeEntry;
use theway_contract::triggers::{CronJob, DynamicTriggerRule};

use super::*;
use crate::trigger_engine::execution::{
    NotificationStatusSnapshot, RunningTriggerState,
};
use crate::trigger_engine::notification_hook::{HookState, NotificationHookStatus};
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;

fn dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn cron_job(id: &str) -> CronJob {
    CronJob {
        id: id.into(),
        schedule: "*/5 * * * *".into(),
        action: "echo hi".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: dt("2026-05-22T19:00:00Z"),
    }
}

fn hook(state: HookState) -> NotificationHookStatus {
    NotificationHookStatus {
        state,
        last_event_at: None,
        last_ack_at: None,
        last_error: None,
        queued_count: 0,
        dropped_count: 0,
        deduped_count: 0,
        subscription_labels: vec![],
        requires_attention: None,
    }
}

#[test]
fn render_cron_jobs_empty_returns_none_line() {
    assert_eq!(render_cron_jobs(&[]), vec!["Cron jobs (session): none"]);
}

#[test]
fn render_cron_jobs_renders_all_fields() {
    let mut running = cron_job("job-1");
    running.running_trace_id = Some("trace-1".into());
    running.stateful = true;
    running.skipped_overlap_count = 4;
    running.last_error = Some("bad schedule".into());

    let mut fired = cron_job("job-2");
    fired.enabled = false;
    fired.last_fired_at = Some(dt("2026-05-22T19:30:00Z"));

    let lines = render_cron_jobs(&[running, fired]);
    let text = lines.join("\n");
    assert!(text.contains("Cron jobs (session, 2):"), "{text}");
    assert!(text.contains("job-1  enabled  */5 * * * *  [stateful], running trace-1"), "{text}");
    assert!(text.contains("overlap skips: 4"), "{text}");
    assert!(text.contains("last: bad schedule"), "{text}");
    assert!(text.contains("job-2  disabled"), "{text}");
    assert!(text.contains("last fired: 2026-05-22T19:30:00+00:00"), "{text}");
}

#[test]
fn preview_cron_action_redacts_and_truncates() {
    let short = preview_cron_action("echo hi");
    assert_eq!(short, "echo hi");

    let long = "x".repeat(200);
    let preview = preview_cron_action(&long);
    assert_eq!(preview.chars().count(), 121, "{preview}");
    assert!(preview.ends_with('…'), "{preview}");
}

#[test]
fn preview_cron_text_truncates_at_boundary() {
    assert_eq!(preview_cron_text("abc", 3), "abc");
    assert_eq!(preview_cron_text("abcd", 3), "abc…");
}

#[test]
fn render_triggers_status_renders_empty_runtime() {
    let snapshot = NotificationStatusSnapshot {
        hooks: vec![],
        runtime: TriggerRuntimeSnapshot {
            dedup_entries: 0,
            active_traces: 0,
            accepted_total: 0,
            deduped_total: 0,
            cycle_suppressed_total: 0,
        },
        running: vec![],
    };
    let lines = render_triggers_status(&snapshot);
    let text = lines.join("\n");
    assert!(text.contains("dynamic rules: 0 total"), "{text}");
    assert!(text.contains("engine: accepted=0"), "{text}");
    assert!(text.contains("sources: 0 total"), "{text}");
}

#[test]
fn render_dynamic_trigger_rules_handles_empty_limit_and_more() {
    assert_eq!(
        render_dynamic_trigger_rules(&[], 3),
        vec!["Dynamic trigger rules: none"]
    );

    let rules = vec![
        DynamicTriggerRule {
            id: "dyn-1".into(),
            condition: "cond 1".into(),
            action: "act 1".into(),
            enabled: true,
            fire_once: true,
            fired_at: Some(dt("2026-05-22T19:00:00Z")),
            promote_to_chat: true,
            created_at: dt("2026-05-22T19:00:00Z"),
        },
        DynamicTriggerRule {
            id: "dyn-2".into(),
            condition: "cond 2".into(),
            action: "act 2".into(),
            enabled: false,
            fire_once: false,
            fired_at: None,
            promote_to_chat: false,
            created_at: dt("2026-05-22T19:00:00Z"),
        },
        DynamicTriggerRule {
            id: "dyn-3".into(),
            condition: "cond 3".into(),
            action: "act 3".into(),
            enabled: true,
            fire_once: false,
            fired_at: None,
            promote_to_chat: false,
            created_at: dt("2026-05-22T19:00:00Z"),
        },
    ];

    let lines = render_dynamic_trigger_rules(&rules, 2);
    let text = lines.join("\n");
    assert!(text.contains("Dynamic trigger rules (3):"), "{text}");
    assert!(text.contains("dyn-1 [enabled, fire_once, promote_to_chat, fired_at=2026-05-22T19:00:00+00:00]"), "{text}");
    assert!(text.contains("dyn-2 [disabled, repeat, audit_only]"), "{text}");
    assert!(text.contains("... 1 more; run /triggers rules"), "{text}");
    assert!(!text.contains("dyn-3"), "{text}");
}

#[test]
fn render_trigger_sources_handles_empty_and_every_hook_state() {
    assert_eq!(render_trigger_sources(&[]), vec!["(no trigger sources registered)"]);

    let hooks = vec![
        NotificationHookStatus {
            state: HookState::Connected,
            last_event_at: Some(dt("2026-05-22T19:00:00Z")),
            last_ack_at: None,
            last_error: None,
            queued_count: 1,
            dropped_count: 2,
            deduped_count: 3,
            subscription_labels: vec!["repo x".into(), "repo y".into()],
            requires_attention: None,
        },
        NotificationHookStatus {
            state: HookState::Reconnecting,
            last_event_at: None,
            last_ack_at: None,
            last_error: Some("handshake failed".into()),
            queued_count: 4,
            dropped_count: 5,
            deduped_count: 6,
            subscription_labels: vec![],
            requires_attention: Some("upgrade hub".into()),
        },
        NotificationHookStatus {
            state: HookState::Disabled,
            last_event_at: None,
            last_ack_at: None,
            last_error: None,
            queued_count: 0,
            dropped_count: 0,
            deduped_count: 0,
            subscription_labels: vec![],
            requires_attention: None,
        },
        NotificationHookStatus {
            state: HookState::AuthFailed { reason: "bad token".into() },
            last_event_at: None,
            last_ack_at: None,
            last_error: None,
            queued_count: 0,
            dropped_count: 0,
            deduped_count: 0,
            subscription_labels: vec![],
            requires_attention: None,
        },
        NotificationHookStatus {
            state: HookState::Disconnected { reason: "protocol_mismatch".into() },
            last_event_at: None,
            last_ack_at: None,
            last_error: None,
            queued_count: 0,
            dropped_count: 0,
            deduped_count: 0,
            subscription_labels: vec![],
            requires_attention: None,
        },
    ];

    let lines = render_trigger_sources(&hooks);
    let text = lines.join("\n");
    assert!(text.contains("Trigger sources (5):"), "{text}");
    assert!(text.contains("source #1: connected queued=1 dropped=2 deduped=3 last_event=2026-05-22T19:00:00+00:00"), "{text}");
    assert!(text.contains("subscriptions: repo x, repo y"), "{text}");
    assert!(text.contains("source #2: reconnecting queued=4 dropped=5 deduped=6 last_event=never"), "{text}");
    assert!(text.contains("attention: upgrade hub"), "{text}");
    assert!(text.contains("last error: handshake failed"), "{text}");
    assert!(text.contains("source #3: disabled"), "{text}");
    assert!(text.contains("source #4: auth_failed (bad token)"), "{text}");
    assert!(text.contains("source #5: disconnected (protocol_mismatch)"), "{text}");
}

#[test]
fn render_hook_state_covers_all_variants() {
    assert_eq!(render_hook_state(&HookState::Connected), "connected");
    assert_eq!(render_hook_state(&HookState::Reconnecting), "reconnecting");
    assert_eq!(
        render_hook_state(&HookState::Disconnected { reason: "r".into() }),
        "disconnected (r)"
    );
    assert_eq!(render_hook_state(&HookState::Disabled), "disabled");
    assert_eq!(
        render_hook_state(&HookState::AuthFailed { reason: "r".into() }),
        "auth_failed (r)"
    );
}

#[test]
fn render_requires_attention_renders_message_or_empty() {
    let mut h = hook(HookState::Connected);
    assert_eq!(render_requires_attention(&h), "");
    h.requires_attention = Some("upgrade hub".into());
    assert_eq!(render_requires_attention(&h), "  attention: upgrade hub");
}

#[test]
fn render_running_triggers_handles_empty_and_running() {
    assert_eq!(render_running_triggers(&[]), vec!["(no running triggers)"]);

    let running = vec![RunningTriggerState {
        trace_id: "trace-1".into(),
        source_label: "mcp:github".into(),
        event_label: "pr_merged".into(),
        started_at: dt("2026-05-22T19:00:00Z"),
        prompt_preview: "summarize release".into(),
    }];
    let lines = render_running_triggers(&running);
    let text = lines.join("\n");
    assert!(text.contains("Running triggers (1):"), "{text}");
    assert!(text.contains("trace-1  mcp:github / pr_merged  since 2026-05-22T19:00:00+00:00"), "{text}");
    assert!(text.contains("summarize release"), "{text}");
}

#[test]
fn render_trigger_audit_handles_empty_rows() {
    assert_eq!(
        render_trigger_audit(&[]),
        vec!["(no trigger audit entries in this session)"]
    );
}

#[test]
fn collect_trigger_audit_rows_skips_non_trigger_entries() {
    let entries = vec![SessionTreeEntry::Custom {
        id: "x".into(),
        parent_id: None,
        timestamp: "2026-05-22T19:00:00Z".into(),
        custom_type: "not_trigger".into(),
        data: None,
    }];
    let rows = collect_trigger_audit_rows(&entries, 10);
    assert!(rows.is_empty());
}

#[test]
fn trigger_decision_details_handles_missing_decision_and_unknown_outcome() {
    let missing = trigger_decision_details(&serde_json::json!({}));
    assert!(missing.is_empty());

    let present = trigger_decision_details(&serde_json::json!({
        "evaluator_decision": {"no_outcome": true}
    }))
    .join("\n");
    assert_eq!(present, "decision: present");

    let unknown = trigger_decision_details(&serde_json::json!({
        "evaluator_decision": {"outcome": "rejected", "hop_count": 2}
    }))
    .join("\n");
    assert_eq!(unknown, "decision: rejected");
}

#[test]
fn trigger_result_and_promotion_details_render_bounded_fields() {
    let result = trigger_result_details(&serde_json::json!({
        "branch_id": "branch-1",
        "message_count": 3,
        "secret": "never-render",
    }))
    .join("\n");
    assert!(result.contains("branch_id: branch-1"), "{result}");
    assert!(result.contains("message_count: 3"), "{result}");
    assert!(!result.contains("never-render"), "{result}");

    let promotion = trigger_promotion_details(&serde_json::json!({
        "promote_kind": "inject_summary",
        "inserted_entry_id": "entry-9",
        "secret": "never-render",
    }))
    .join("\n");
    assert!(promotion.contains("promote_kind: inject_summary"), "{promotion}");
    assert!(promotion.contains("inserted_entry_id: entry-9"), "{promotion}");
    assert!(!promotion.contains("never-render"), "{promotion}");
}

#[test]
fn string_field_and_number_field_parse_scalar_json_values() {
    let data = serde_json::json!({
        "s": "text",
        "n": 7,
        "s_num": 9,
        "n_text": "9",
    });
    assert_eq!(string_field(&data, "s").as_deref(), Some("text"));
    assert_eq!(string_field(&data, "s_num"), None);
    assert_eq!(number_field(&data, "n"), Some(7));
    assert_eq!(number_field(&data, "n_text"), None);
}
