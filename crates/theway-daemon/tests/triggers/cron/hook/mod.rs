//! Tests for `triggers::cron::hook` — split out of src (see docs/rust-test-files.md).

use super::*;
use chrono::{TimeZone, Utc};
use serde_json::json;

use crate::trigger_engine::event::TriggerEvent;
use crate::trigger_engine::runtime::TriggerRuntimeSnapshot;

fn runtime_snapshot() -> TriggerRuntimeSnapshot {
    TriggerRuntimeSnapshot {
        dedup_entries: 0,
        active_traces: 0,
        accepted_total: 0,
        deduped_total: 0,
        cycle_suppressed_total: 0,
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

fn cron_trigger(job: &CronJob) -> Trigger {
    cron_trigger_for_job(job, Utc::now(), "trace-cron".into())
}

#[test]
fn notification_hook_metadata_and_status_reflect_jobs() {
    let registry = CronRegistry::new();
    let job_a = registry.add_job("* * * * *", "do a").unwrap();
    let job_b = registry.add_job("* * * * *", "do b").unwrap();
    registry.set_job_enabled(&job_b.id, false).unwrap();
    let hook = CronNotificationHook::new(registry.clone());

    assert_eq!(hook.label(), "cron");
    let status = hook.status();
    assert_eq!(status.queued_count, 0);
    assert!(status.subscription_labels[0].contains("2 job(s)"), "{:?}", status.subscription_labels);
    assert!(status.subscription_labels[0].contains("1 enabled"), "{:?}", status.subscription_labels);

    // Fire job_a so it has a running trace; status should count it.
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    let due = registry.due_jobs(since, now);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0.id, job_a.id);

    let status = hook.status();
    assert_eq!(status.queued_count, 1);
}

#[tokio::test]
async fn action_hook_falls_through_for_non_cron_sources() {
    let registry = CronRegistry::new();
    let hook = cron_action_hook(registry, inner_hook());

    let trigger = Trigger {
        source: TriggerSource::Local {
            subkind: "not-cron".into(),
        },
        source_kind: SourceKind::Local,
        source_label: "local:other".into(),
        event_label: "something".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: None,
        payload: None,
        idempotency_key: "k".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-1".into(),
        authority: TriggerAuthority {
            principal_id: "local".into(),
            principal_label: "local".into(),
            credential_scope: CredentialScope::None,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: Utc::now(),
    };

    let action = hook(context(trigger), CancellationToken::new()).await;

    assert_eq!(action.prompt, "inner-hook");
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
}

#[tokio::test]
async fn action_hook_uses_default_when_job_id_missing_or_unknown() {
    let registry = CronRegistry::new();
    let hook = cron_action_hook(registry, inner_hook());

    // Cron source but no `job_id` in payload.
    let missing_job_id = cron_trigger_for_job(
        &CronJob {
            id: "cron-1".into(),
            schedule: "* * * * *".into(),
            action: "x".into(),
            enabled: true,
            running_trace_id: None,
            last_due_at: None,
            last_fired_at: None,
            last_completed_at: None,
            last_error: None,
            skipped_overlap_count: 0,
            stateful: false,
            created_at: Utc::now(),
        },
        Utc::now(),
        "trace-1".into(),
    );
    let mut trigger = missing_job_id;
    trigger.payload = Some(json!({ "not_job_id": "nope" }));
    let action = hook(context(trigger), CancellationToken::new()).await;
    assert_eq!(action.prompt, "Cron fired: cron-1");
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);

    // `job_id` points at a job that does not exist.
    let unknown_job = Trigger {
        source: TriggerSource::Local {
            subkind: "cron".into(),
        },
        source_kind: SourceKind::Local,
        source_label: "Cron".into(),
        event_label: "cron-unknown".into(),
        payload_visibility: PayloadVisibility::Local,
        payload_summary: None,
        payload: Some(json!({ "job_id": "cron-unknown" })),
        idempotency_key: "k".into(),
        replacement_policy: ReplacementPolicy::Drop,
        trace_id: "trace-1".into(),
        authority: TriggerAuthority {
            principal_id: "local-cron".into(),
            principal_label: "local cron".into(),
            credential_scope: CredentialScope::None,
            allowed_source_actions: vec![],
            expires_at: None,
        },
        received_at: Utc::now(),
    };
    let action = hook(context(unknown_job), CancellationToken::new()).await;
    assert_eq!(action.prompt, "Cron fired: cron-unknown");
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
}

#[tokio::test]
async fn action_hook_stateful_job_injects_loop_state_and_uses_subagent() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = dir.path().join("sess1.cron.toml");
    let registry = CronRegistry::new();
    registry.load_from_path(&sidecar).unwrap();
    let job = registry
        .add_job_full("* * * * *", "watch things", true)
        .unwrap();
    let hook = cron_action_hook(registry, inner_hook());

    // First run: no persisted state yet → first-run marker.
    let action = hook(context(cron_trigger(&job)), CancellationToken::new()).await;
    assert_eq!(action.delivery, TriggerDelivery::SubAgent);
    assert!(action.prompt.contains("[loop-state]"), "{:?}", action.prompt);
    assert!(action.prompt.contains("(first run)"), "{:?}", action.prompt);
    assert!(action.prompt.contains("watch things"), "{:?}", action.prompt);
    assert!(matches!(action.promote, PromoteAction::None));

    // Second run: persisted state is injected.
    let state_path = loop_state_path(&sidecar, &job.id);
    write_loop_state(&state_path, "baseline: #1 #2").unwrap();
    let action = hook(context(cron_trigger(&job)), CancellationToken::new()).await;
    assert!(action.prompt.contains("baseline: #1 #2"), "{:?}", action.prompt);
}

#[test]
fn listener_marks_failed_job_completed_with_error() {
    let registry = CronRegistry::new();
    registry.add_job("* * * * *", "do work").unwrap();
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    let trace_id = registry.due_jobs(since, now)[0]
        .0
        .running_trace_id
        .clone()
        .unwrap();

    let listener = cron_trigger_listener(
        registry.clone(),
        std::env::temp_dir().join("unused-inbox.jsonl"),
    );
    listener(TriggerEvent::TriggerFailed {
        trace_id,
        reason: "agent loop failed".into(),
    });

    let job = registry.list().remove(0);
    assert!(job.running_trace_id.is_none());
    assert_eq!(job.last_error.as_deref(), Some("agent loop failed"));
    assert!(job.last_completed_at.is_some());
}

#[test]
fn strip_loop_protocol_tags_removes_blocks_and_collapses_blanks() {
    let text = "before\n<inbox>finding one</inbox>\n\n<loop-state>seen: a</loop-state>\nafter";
    let stripped = strip_loop_protocol_tags(text);
    assert!(stripped.contains("before"), "{stripped}");
    assert!(stripped.contains("after"), "{stripped}");
    assert!(!stripped.contains("<inbox>"), "{stripped}");
    assert!(!stripped.contains("<loop-state>"), "{stripped}");
    assert!(!stripped.contains("finding one"), "{stripped}");

    let untouched = "plain multi-line\nsummary text";
    assert_eq!(strip_loop_protocol_tags(untouched), untouched);

    // Unclosed tag leaves the text as-is (after stripping any previous closed tags).
    let unclosed = "<loop-state>never closed";
    assert!(strip_loop_protocol_tags(unclosed).contains("<loop-state>"));
}

#[test]
fn loop_state_path_falls_back_to_session_stem_for_other_names() {
    let dir = tempfile::tempdir().unwrap();
    let sidecar = dir.path().join("arbitrary-name.toml");
    let path = loop_state_path(&sidecar, "cron-1234567890abcdef");
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        "session.loop-cron-12345678.md"
    );
}

#[test]
fn cron_trigger_payload_carries_job_id_and_due_at() {
    let registry = CronRegistry::new();
    let job = registry.add_job("*/5 * * * *", "echo hi").unwrap();
    let due_at = Utc.with_ymd_and_hms(2026, 5, 26, 22, 5, 0).unwrap();

    let trigger = cron_trigger_for_job(&job, due_at, "trace-1".into());

    assert_eq!(trigger.source_label, "Cron");
    assert_eq!(trigger.event_label, job.id);
    assert_eq!(trigger.idempotency_key, format!("cron:{}:{}", job.id, due_at.to_rfc3339()));
    assert_eq!(trigger.trace_id, "trace-1");
    assert_eq!(trigger.authority.principal_id, "local-cron");
    let payload = trigger.payload.unwrap();
    assert_eq!(payload["job_id"], job.id);
    assert_eq!(payload["due_at"], due_at.to_rfc3339());
}
