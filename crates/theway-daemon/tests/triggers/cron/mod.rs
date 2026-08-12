//! Tests for `cron` — split out of src (see docs/RUST_TEST_FILES.md).

use super::*;
use chrono::TimeZone;
use tempfile::tempdir;
use crate::trigger_engine::types::TriggerRecord;

#[test]
fn cron_parser_supports_steps_ranges_and_sunday_alias() {
    let expr = CronExpression::parse("*/15 9-17 * * 1,7").unwrap();
    assert!(expr.minutes.contains(&0));
    assert!(expr.minutes.contains(&45));
    assert!(expr.hours.contains(&9));
    assert!(expr.hours.contains(&17));
    assert!(expr.days_of_week.contains(&0));
    assert!(expr.days_of_week.contains(&1));
}

#[test]
fn cron_parser_rejects_invalid_schedule() {
    assert!(CronExpression::parse("* * * *").is_err());
    assert!(CronExpression::parse("60 * * * *").is_err());
    assert!(CronExpression::parse("*/0 * * * *").is_err());
}

#[test]
fn next_after_uses_local_time_and_does_not_return_current_minute() {
    let expr = CronExpression::parse("5 * * * *").unwrap();
    let base = Utc.with_ymd_and_hms(2026, 5, 26, 22, 5, 0).unwrap();
    let next = expr.next_after(base).unwrap();
    let local = next.with_timezone(&Local);
    assert_eq!(local.minute(), 5);
    assert!(next > base);
}

#[test]
fn registry_round_trips_storage_and_enable_state() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cron.toml");
    let registry = CronRegistry::new();
    registry.load_from_path(&path).unwrap();
    let job = registry.add_job("*/10 * * * *", "say hello").unwrap();
    registry.set_job_enabled(&job.id, false).unwrap();

    let reloaded = CronRegistry::new();
    reloaded.load_from_path(&path).unwrap();
    let jobs = reloaded.list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "*/10 * * * *");
    assert_eq!(jobs[0].action, "say hello");
    assert!(!jobs[0].enabled);
}

#[test]
fn tag_extraction_handles_present_absent_truncated_and_caps() {
    let text = "did work\n<inbox>finding one</inbox>\nmore\n<inbox>finding two</inbox>\n<loop-state>seen: a,b</loop-state>";
    assert_eq!(
        extract_tag_block(text, "loop-state").as_deref(),
        Some("seen: a,b")
    );
    assert_eq!(
        extract_tag_all(text, "inbox", 16),
        vec!["finding one".to_string(), "finding two".to_string()]
    );
    assert_eq!(extract_tag_block("no tags here", "loop-state"), None);
    // Truncated open tag (summary cap can cut mid-stream): fail quiet.
    assert_eq!(
        extract_tag_block("x <loop-state>cut off", "loop-state"),
        None
    );
    // Cap honored.
    let many: String = (0..30).map(|i| format!("<inbox>f{i}</inbox>")).collect();
    assert_eq!(extract_tag_all(&many, "inbox", 16).len(), 16);
}

#[test]
fn stateful_prompt_injects_previous_state_and_protocol() {
    let prompt = compose_stateful_prompt("check the issues", Some("baseline: #1 #2"));
    assert!(prompt.contains("[loop-state]"), "{prompt}");
    assert!(prompt.contains("baseline: #1 #2"), "{prompt}");
    assert!(prompt.contains("check the issues"), "{prompt}");
    assert!(
        prompt.contains("<loop-state>"),
        "protocol instructions: {prompt}"
    );
    assert!(
        prompt.contains("<inbox>"),
        "protocol instructions: {prompt}"
    );
    let first = compose_stateful_prompt("check", None);
    assert!(first.contains("(first run)"), "{first}");
}

#[test]
fn loop_state_paths_and_write_cap() {
    let dir = tempdir().unwrap();
    let sidecar = dir.path().join("019abc.cron.toml");
    let path = loop_state_path(&sidecar, "cron-1234567890abcdef");
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        "019abc.loop-cron-12345678.md"
    );
    write_loop_state(&path, &"x".repeat(5000)).unwrap();
    let read = read_loop_state(&path).unwrap();
    assert!(read.chars().count() <= 2001, "state capped");
    assert!(read_loop_state(&dir.path().join("missing.md")).is_none());
}

#[test]
fn listener_persists_state_and_inbox_for_stateful_job_completion() {
    let dir = tempdir().unwrap();
    let sidecar = dir.path().join("sess1.cron.toml");
    let inbox_path = dir.path().join("inbox.jsonl");
    let registry = CronRegistry::new();
    registry.load_from_path(&sidecar).unwrap();
    let job = registry
        .add_job_full("* * * * *", "watch things", true)
        .unwrap();
    assert!(job.stateful);
    // Fire it so a running trace exists (listener resolves trace -> job).
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    let due = registry.due_jobs(since, now);
    let trace_id = due[0].0.running_trace_id.clone().unwrap();

    let listener = cron_trigger_listener(registry.clone(), inbox_path.clone());
    listener(TriggerEvent::TriggerCompleted {
        trace_id: trace_id.clone(),
        summary: Some(
            "checked. <inbox>issue #9 looks stuck</inbox> done <loop-state>seen: #9</loop-state>"
                .into(),
        ),
        cost_usd: None,
        details: serde_json::Value::Null,
    });

    let state_path = loop_state_path(&sidecar, &job.id);
    assert_eq!(read_loop_state(&state_path).as_deref(), Some("seen: #9"));
    let entries = theway_transport::inbox::list_new(&inbox_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].text.contains("issue #9"), "{:?}", entries[0]);
    assert!(entries[0].source.starts_with("cron:"), "{:?}", entries[0]);
    assert_eq!(entries[0].trace_id, trace_id);
    // Job marked completed (running state cleared).
    assert!(registry.list()[0].running_trace_id.is_none());
}

#[test]
fn due_jobs_tick_writes_sidecar_only_when_state_changed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cron.toml");
    let registry = CronRegistry::new();
    registry.load_from_path(&path).unwrap();
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();

    // Empty registry: an idle tick must not create the sidecar.
    assert!(registry.due_jobs(since, now).is_empty());
    assert!(!path.exists(), "idle tick created an empty sidecar");

    // Job exists but is not due: tick must not rewrite the file.
    registry.add_job("0 0 1 1 *", "yearly job").unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(registry.due_jobs(since, now).is_empty());
    assert!(!path.exists(), "no-op tick rewrote the sidecar");

    // A due job is a real state change and must persist.
    registry.add_job("* * * * *", "every minute").unwrap();
    assert_eq!(registry.due_jobs(since, now).len(), 1);
    assert!(path.exists(), "firing tick must persist job state");
}

#[test]
fn load_clears_stale_running_state_from_previous_process() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cron.toml");
    let registry = CronRegistry::new();
    registry.load_from_path(&path).unwrap();
    let job = registry.add_job("* * * * *", "say hello").unwrap();
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    assert_eq!(registry.due_jobs(since, now).len(), 1);
    assert!(registry.list()[0].running_trace_id.is_some());

    let reloaded = CronRegistry::new();
    reloaded.load_from_path(&path).unwrap();
    let jobs = reloaded.list();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].id, job.id);
    assert!(jobs[0].running_trace_id.is_none());
    assert_eq!(
        jobs[0].last_error.as_deref(),
        Some("cleared stale running state on startup")
    );

    let persisted = read_jobs_file(&path).unwrap();
    assert!(persisted[0].running_trace_id.is_none());
}

#[test]
fn registry_rejects_oversized_action() {
    let registry = CronRegistry::new();
    let err = registry
        .add_job("* * * * *", &"x".repeat(MAX_ACTION_BYTES + 1))
        .unwrap_err();
    assert!(matches!(err, AddCronJobError::ActionTooLarge { .. }));
}

#[test]
fn trigger_summary_redacts_secret_like_action_text() {
    let registry = CronRegistry::new();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    let bearer = "Bearer abcdefghijklmnopqrstuvwxyz";
    let job = registry
        .add_job("* * * * *", &format!("use token {secret} and {bearer}"))
        .unwrap();
    let trigger = cron_trigger_for_job(&job, Utc::now(), "trace-cron".into());
    let record = TriggerRecord::received_from(&trigger);
    let summary = record.payload_summary.unwrap();
    assert!(!summary.contains(secret), "{summary}");
    assert!(!summary.contains(bearer), "{summary}");
    assert!(summary.contains("[REDACTED:"), "{summary}");
}

#[test]
fn due_jobs_marks_running_and_skips_overlap() {
    let registry = CronRegistry::new();
    let job = registry.add_job("* * * * *", "do work").unwrap();
    let since = Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 26, 22, 1, 5).unwrap();
    let due = registry.due_jobs(since, now);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0.id, job.id);
    assert!(registry.list()[0].running_trace_id.is_some());

    let later = Utc.with_ymd_and_hms(2026, 5, 26, 22, 2, 5).unwrap();
    let skipped = registry.due_jobs(now, later);
    assert!(skipped.is_empty());
    assert_eq!(registry.list()[0].skipped_overlap_count, 1);
}

#[test]
fn listener_clears_running_job_by_trace_id() {
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
    listener(TriggerEvent::TriggerCompleted {
        trace_id,
        summary: None,
        cost_usd: None,
        details: serde_json::Value::Null,
    });
    let job = registry.list().remove(0);
    assert!(job.running_trace_id.is_none());
    assert!(job.last_completed_at.is_some());
}

#[tokio::test]
async fn cron_action_hook_maps_cron_trigger_to_inject_and_run() {
    let registry = CronRegistry::new();
    let job = registry.add_job("* * * * *", "run tests").unwrap();
    let trigger = cron_trigger_for_job(&job, Utc::now(), "trace-cron".into());
    let inner: BeforeTriggerActionHook =
        Arc::new(|ctx, _cancel| Box::pin(async move { TriggerAction::default_for(&ctx.trigger) }));
    let hook = cron_action_hook(registry, inner);
    let action = hook(
        BeforeTriggerActionContext {
            trigger,
            runtime: crate::trigger_engine::runtime::TriggerRuntimeSnapshot {
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
    assert_eq!(action.prompt, "run tests");
    assert_eq!(action.delivery, TriggerDelivery::InjectAndRun);
}
