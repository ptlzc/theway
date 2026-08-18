//! Tests for `triggers::cron::errors` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::sync::Arc;

use chrono::{TimeZone, Utc};

fn sample_job() -> CronJob {
    CronJob {
        id: "cron-sample".into(),
        schedule: "*/5 * * * *".into(),
        action: "run cargo test".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: Utc.with_ymd_and_hms(2026, 5, 26, 22, 0, 0).unwrap(),
    }
}

#[test]
fn cron_expression_parse_rejects_invalid_fields() {
    let cases = [
        ("a * * * *", "`a` is not a number"),
        ("0 24 * * *", "value 24 outside 0-23"),
        ("* * 0 * *", "value 0 outside 1-31"),
        ("* * * 13 *", "value 13 outside 1-12"),
        ("* * * * 8", "value 8 outside 0-7"),
        ("*/x * * * *", "step must be a positive integer"),
        ("1,,2 * * * *", "empty item"),
        ("5-3 * * * *", "range start must be <= range end"),
    ];
    for (input, needle) in cases {
        let err = CronExpression::parse(input)
            .expect_err(&format!("{input} should fail"));
        assert!(
            err.to_string().contains(needle),
            "{input}: got {err:?}, expected {needle}"
        );
    }
}

#[test]
fn cron_expression_parse_accepts_day_of_week_zero_and_seven_as_sunday() {
    let expr = CronExpression::parse("0 0 * * 7").unwrap();
    assert!(expr.days_of_week.contains(&0));
    assert!(!expr.days_of_week.contains(&7));

    let expr = CronExpression::parse("0 0 * * 0").unwrap();
    assert!(expr.days_of_week.contains(&0));
}

#[test]
fn normalize_schedule_accepts_cron_and_aliases() {
    assert_eq!(normalize_schedule(" */5 * * * * ").unwrap(), "*/5 * * * *");
    assert_eq!(normalize_schedule("Hourly").unwrap(), "0 * * * *");
    assert_eq!(normalize_schedule(" every day ").unwrap(), "0 9 * * *");
    assert_eq!(normalize_schedule(" once a week").unwrap(), "0 9 * * 1");
    assert_eq!(normalize_schedule("每小时").unwrap(), "0 * * * *");
    assert_eq!(normalize_schedule("每天").unwrap(), "0 9 * * *");
    assert_eq!(normalize_schedule("每周").unwrap(), "0 9 * * 1");

    let err = normalize_schedule("not a schedule").unwrap_err();
    assert!(err.to_string().contains("invalid schedule"), "{err}");
}

#[test]
fn preview_redacted_redacts_secrets_and_caps_chars() {
    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    let preview = preview_redacted(&format!("token {secret}"), 80);
    assert!(!preview.contains(secret), "{preview}");
    assert!(preview.contains("[REDACTED:"), "{preview}");

    assert_eq!(preview_redacted("hello world", 5), "hello…");
    assert_eq!(preview_redacted("hello", 5), "hello");
    assert_eq!(preview_redacted("hello", 0), "…");
}

#[test]
fn cron_control_plane_audit_reflects_before_after_and_next_run() {
    let after = sample_job();
    let audit = cron_control_plane_audit("add", "tool", None, Some(&after));
    assert_eq!(audit["op"], "add");
    assert_eq!(audit["actor"], "tool");
    assert_eq!(audit["job_id"], "cron-sample");
    assert_eq!(audit["schedule"], "*/5 * * * *");
    assert_eq!(audit["action_preview"], "run cargo test");
    assert!(audit["before_enabled"].is_null());
    assert_eq!(audit["after_enabled"], true);
    assert!(audit["next_run"].is_string(), "{audit}");
    assert_eq!(audit["removed"], false);

    let removed = cron_control_plane_audit("remove", "tool", Some(&after), None);
    assert!(removed["before_enabled"].as_bool().unwrap());
    assert!(removed["after_enabled"].is_null());
    assert!(removed["next_run"].is_null());
    assert_eq!(removed["removed"], true);

    let mut disabled = sample_job();
    disabled.enabled = false;
    let audit = cron_control_plane_audit("disable", "tool", Some(&after), Some(&disabled));
    assert!(audit["next_run"].is_null(), "{audit}");
    assert_eq!(audit["before_enabled"], true);
    assert_eq!(audit["after_enabled"], false);
}

#[test]
fn render_cron_jobs_for_tool_empty_and_populated() {
    assert_eq!(render_cron_jobs_for_tool(&[]), "session cron jobs: none");

    let mut job = sample_job();
    job.running_trace_id = Some("cron-trace".into());
    job.last_error = Some("boom".into());
    job.skipped_overlap_count = 2;
    let rendered = render_cron_jobs_for_tool(std::slice::from_ref(&job));

    assert!(rendered.contains("session cron jobs: 1"), "{rendered}");
    assert!(rendered.contains("- cron-sample [enabled]"), "{rendered}");
    assert!(rendered.contains("schedule: */5 * * * *"), "{rendered}");
    assert!(rendered.contains("action: run cargo test"), "{rendered}");
    assert!(rendered.contains("next_run:"), "{rendered}");
    assert!(rendered.contains("running_trace_id: cron-trace"), "{rendered}");
    assert!(rendered.contains("last_error: boom"), "{rendered}");
    assert!(rendered.contains("skipped_overlap_count: 2"), "{rendered}");

    let mut disabled = sample_job();
    disabled.enabled = false;
    let rendered = render_cron_jobs_for_tool(std::slice::from_ref(&disabled));
    assert!(rendered.contains("[disabled]"), "{rendered}");
    assert!(!rendered.contains("next_run:"), "{rendered}");
}

#[test]
fn cron_job_details_for_model_contains_preview_safe_fields() {
    let job = sample_job();
    let details = cron_job_details_for_model(&job);
    assert_eq!(details["id"], "cron-sample");
    assert_eq!(details["schedule"], "*/5 * * * *");
    assert_eq!(details["action_preview"], "run cargo test");
    assert_eq!(details["enabled"], true);
    assert_eq!(details["scope"], "session");
    assert!(details["running_trace_id"].is_null());
    assert!(details["last_due_at"].is_null());
    assert!(details["last_fired_at"].is_null());
    assert!(details["last_completed_at"].is_null());
    assert!(details["last_error"].is_null());
    assert_eq!(details["skipped_overlap_count"], 0);
    assert!(details["next_run"].is_string(), "{details}");
    assert_eq!(details["created_at"], "2026-05-26T22:00:00+00:00");
}

#[tokio::test]
async fn write_tool_cron_control_audit_handles_missing_and_present_harness() {
    // No harness cell: returns None and never panics.
    let none = write_tool_cron_control_audit(&None, "add", None, None).await;
    assert!(none.is_none());

    // Present harness: writes an audit entry to the session.
    let session = theway_core::Session::new(std::sync::Arc::new(
        theway_core::MemorySessionStorage::new(),
    ) as std::sync::Arc<dyn theway_core::SessionStorage>);
    let harness = std::sync::Arc::new(theway_core::AgentHarness::new(
        theway_core::AgentHarnessOptions::new(
            theway_llm_provider::Model {
                id: "faux".into(),
                name: "Faux".into(),
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                base_url: String::new(),
                reasoning: false,
                thinking_level_map: None,
                input: vec![],
                cost: theway_llm_provider::ModelCost::default(),
                context_window: 0,
                max_tokens: 0,
                headers: None,
                compat: None,
            },
            session.clone(),
        ),
    ));
    let cell = Arc::new(once_cell::sync::OnceCell::new());
    let _ = cell.set(harness.clone());

    let before = sample_job();
    let id = write_tool_cron_control_audit(&Some(cell), "disable", Some(&before), None)
        .await
        .expect("audit should be written");
    assert!(!id.is_empty());

    let entries = session.entries().await.unwrap();
    assert!(entries.iter().any(|entry| {
        matches!(
            entry,
            theway_core::SessionTreeEntry::Custom {
                custom_type,
                data: Some(_),
                ..
            } if custom_type == "cron_control_plane"
        )
    }));
}
