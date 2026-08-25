// ── wire/runtime storage models: serde, defaults, optional fields ────

use crate::wire::{
    WireLoadDagRunsRequest, WireSaveDagRunRequest, WireStoredCronJob, WireStoredDagRun,
    WireStoredTriggerRule,
};

#[test]
fn wire_runtime_dag_run_round_trips_json() {
    let value = WireStoredDagRun {
        session_id: "sess-1".into(),
        run_id: "dag-1".into(),
        snapshot: r#"{"id":"dag-1"}"#.into(),
    };
    let json = serde_json::to_string(&value).unwrap();
    let back: WireStoredDagRun = serde_json::from_str(&json).unwrap();
    assert_eq!(back, value);
}

#[test]
fn wire_runtime_save_dag_run_request_round_trips_json() {
    let request = WireSaveDagRunRequest {
        session_id: "sess-1".into(),
        run_id: "dag-1".into(),
        snapshot: "{}".into(),
    };
    let back: WireSaveDagRunRequest =
        serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();
    assert_eq!(back, request);
}

#[test]
fn wire_runtime_load_dag_runs_request_keeps_optional_run_id() {
    let request = WireLoadDagRunsRequest {
        session_id: "sess-1".into(),
        run_id: Some("dag-1".into()),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("run_id"));
    let back: WireLoadDagRunsRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.run_id.as_deref(), Some("dag-1"));

    let none = WireLoadDagRunsRequest {
        session_id: "sess-1".into(),
        run_id: None,
    };
    let json = serde_json::to_string(&none).unwrap();
    assert!(!json.contains("run_id"));
    let back: WireLoadDagRunsRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.run_id, None);
}

#[test]
fn wire_runtime_trigger_rule_defaults_fire_once_true() {
    let rule = WireStoredTriggerRule::default();
    assert!(rule.fire_once);
    assert!(rule.fired_at.is_none());
    assert!(!rule.promote_to_chat);
    assert!(rule.created_at.is_empty());
}

#[test]
fn wire_runtime_trigger_rule_deserializes_missing_fire_once_as_true() {
    let json = r#"{"id":"tr-1","condition":"c","action":"a","enabled":true,"created_at":"2026-01-01T00:00:00Z"}"#;
    let rule: WireStoredTriggerRule = serde_json::from_str(json).unwrap();
    assert!(rule.fire_once);
    assert!(!rule.promote_to_chat);
    assert_eq!(rule.id, "tr-1");
}

#[test]
fn wire_runtime_trigger_rule_round_trips_optional_fields() {
    let rule = WireStoredTriggerRule {
        id: "tr-1".into(),
        condition: "file changed".into(),
        action: "run tests".into(),
        enabled: true,
        fire_once: false,
        fired_at: Some("2026-01-02T00:00:00Z".into()),
        promote_to_chat: true,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&rule).unwrap();
    let back: WireStoredTriggerRule = serde_json::from_str(&json).unwrap();
    assert_eq!(back, rule);
}

#[test]
fn wire_runtime_cron_job_round_trips_optional_fields_and_defaults() {
    let job = WireStoredCronJob {
        id: "cron-1".into(),
        schedule: "*/5 * * * *".into(),
        action: "backup".into(),
        enabled: true,
        running_trace_id: Some("trace-1".into()),
        last_due_at: Some("2026-01-01T00:00:00Z".into()),
        last_fired_at: Some("2026-01-01T00:01:00Z".into()),
        last_completed_at: Some("2026-01-01T00:02:00Z".into()),
        last_error: Some("boom".into()),
        skipped_overlap_count: 3,
        stateful: true,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&job).unwrap();
    let back: WireStoredCronJob = serde_json::from_str(&json).unwrap();
    assert_eq!(back, job);
}

#[test]
fn wire_runtime_cron_job_deserializes_missing_optionals() {
    let json = r#"{"id":"cron-1","schedule":"* * * * *","action":"a","enabled":false,"created_at":"2026-01-01T00:00:00Z"}"#;
    let job: WireStoredCronJob = serde_json::from_str(json).unwrap();
    assert_eq!(job.running_trace_id, None);
    assert_eq!(job.last_error, None);
    assert_eq!(job.skipped_overlap_count, 0);
    assert!(!job.stateful);
}
