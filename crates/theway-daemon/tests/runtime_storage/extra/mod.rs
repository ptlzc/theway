//! Additional mirrored coverage for `runtime_storage` — sidecar path errors
//! and the remaining wire-conversion timestamp branches.

use chrono::{DateTime, Utc};
use theway_transport::triggers::{CronJob, DynamicTriggerRule};
use theway_transport::wire::{WireStoredCronJob, WireStoredTriggerRule};

use super::super::*;

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn dynamic_rule(id: &str, fired_at: Option<DateTime<Utc>>) -> DynamicTriggerRule {
    DynamicTriggerRule {
        id: id.to_string(),
        condition: "file_count > 1".to_string(),
        action: "notify".to_string(),
        enabled: false,
        fire_once: false,
        fired_at,
        promote_to_chat: false,
        created_at: dt("2026-01-01T00:00:00Z"),
    }
}

fn cron_job(id: &str) -> CronJob {
    CronJob {
        id: id.to_string(),
        schedule: "*/5 * * * *".to_string(),
        action: "run".to_string(),
        enabled: false,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: None,
        skipped_overlap_count: 0,
        stateful: false,
        created_at: dt("2026-01-01T00:00:00Z"),
    }
}

#[tokio::test]
async fn local_sidecar_path_errors_when_session_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();

    let err = local_sidecar_path(&cwd, "missing-session", SidecarKind::Trigger)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("missing-session"), "{err}");
}

#[test]
fn trigger_to_wire_none_fired_at_round_trips() {
    let rule = dynamic_rule("rule-1", None);

    let wire = trigger_to_wire(&rule);
    let back = trigger_from_wire(&wire).unwrap();

    assert_eq!(wire.fired_at, None);
    assert_eq!(back, rule);
}

#[test]
fn trigger_from_wire_rejects_invalid_created_at() {
    let wire = WireStoredTriggerRule {
        id: "rule-1".into(),
        condition: "c".into(),
        action: "a".into(),
        enabled: true,
        fire_once: true,
        fired_at: None,
        promote_to_chat: false,
        created_at: "not-a-time".into(),
    };

    let err = trigger_from_wire(&wire).unwrap_err();

    assert!(err.to_string().contains("invalid RFC3339"), "{err}");
}

#[test]
fn cron_to_wire_none_optional_timestamps_round_trip() {
    let job = cron_job("job-1");

    let wire = cron_to_wire(&job);
    let back = cron_from_wire(&wire).unwrap();

    assert_eq!(wire.running_trace_id, None);
    assert_eq!(wire.last_due_at, None);
    assert_eq!(wire.last_fired_at, None);
    assert_eq!(wire.last_completed_at, None);
    assert_eq!(wire.last_error, None);
    assert_eq!(back, job);
}

#[test]
fn cron_from_wire_rejects_invalid_optional_timestamps() {
    for invalid_field in ["last_due_at", "last_fired_at", "last_completed_at"] {
        let mut wire = WireStoredCronJob {
            id: "job-1".into(),
            schedule: "*/5 * * * *".into(),
            action: "run".into(),
            enabled: true,
            running_trace_id: None,
            last_due_at: None,
            last_fired_at: None,
            last_completed_at: None,
            last_error: None,
            skipped_overlap_count: 0,
            stateful: false,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        match invalid_field {
            "last_due_at" => wire.last_due_at = Some("not-a-time".into()),
            "last_fired_at" => wire.last_fired_at = Some("not-a-time".into()),
            "last_completed_at" => wire.last_completed_at = Some("not-a-time".into()),
            _ => unreachable!(),
        }

        let err = cron_from_wire(&wire).unwrap_err();

        assert!(
            err.to_string().contains("invalid RFC3339"),
            "{invalid_field}: {err}"
        );
    }
}

#[test]
fn parse_rfc3339_parses_valid_timestamps() {
    let parsed = parse_rfc3339("2026-01-02T03:04:05Z").unwrap();

    assert_eq!(parsed, dt("2026-01-02T03:04:05Z"));
}
