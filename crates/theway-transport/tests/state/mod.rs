//! Tests for `state` — split out of src (see docs/rust-test-files.md).
//!
//! Codec round-trips (wire ↔ proto) for every `state.proto` `StorageService`
//! message family: DAG run save/load, trigger-rule save/load, cron-job
//! save/load.

use super::*;
use crate::wire::{
    WireLoadCronJobsRequest, WireLoadCronJobsResult, WireLoadDagRunsRequest,
    WireLoadDagRunsResult, WireLoadTriggerRulesRequest, WireLoadTriggerRulesResult,
    WireSaveCronJobsRequest, WireSaveCronJobsResult, WireSaveDagRunRequest,
    WireSaveDagRunResult, WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult,
    WireStoredCronJob, WireStoredDagRun, WireStoredTriggerRule,
};

#[test]
fn dag_run_request_response_round_trips_wire_and_proto() {
    let request = WireSaveDagRunRequest {
        session_id: "sess-1".into(),
        run_id: "dag-1".into(),
        snapshot: r#"{"id":"dag-1","name":"build"}"#.into(),
    };
    let proto = save_dag_run_request_to_proto(&request);
    assert_eq!(proto.session_id, "sess-1");
    assert_eq!(proto.run_id, "dag-1");
    assert_eq!(save_dag_run_request_from_proto(&proto), request);

    let result = WireSaveDagRunResult { saved: true };
    let proto = save_dag_run_response_to_proto(&result);
    assert!(proto.saved);
    assert_eq!(save_dag_run_response_from_proto(&proto), result);

    let load = WireLoadDagRunsRequest {
        session_id: "sess-1".into(),
        run_id: Some("dag-1".into()),
    };
    let proto = load_dag_runs_request_to_proto(&load);
    assert_eq!(proto.run_id.as_deref(), Some("dag-1"));
    assert_eq!(load_dag_runs_request_from_proto(&proto), load);

    let stored = WireStoredDagRun {
        session_id: "sess-1".into(),
        run_id: "dag-1".into(),
        snapshot: "{}".into(),
    };
    let proto = stored_dag_run_to_proto(&stored);
    assert_eq!(proto.run_id, "dag-1");
    assert_eq!(stored_dag_run_from_proto(&proto), stored);

    let loaded = WireLoadDagRunsResult {
        runs: vec![stored],
    };
    let proto = load_dag_runs_response_to_proto(&loaded);
    assert_eq!(proto.runs.len(), 1);
    assert_eq!(load_dag_runs_response_from_proto(&proto), loaded);
}

#[test]
fn trigger_rules_round_trip_wire_and_proto() {
    let rule = WireStoredTriggerRule {
        id: "tr-1".into(),
        condition: "file changes".into(),
        action: "run test".into(),
        enabled: true,
        fire_once: false,
        fired_at: Some("2026-01-01T00:00:00Z".into()),
        promote_to_chat: true,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let request = WireSaveTriggerRulesRequest {
        session_id: "sess-1".into(),
        rules: vec![rule.clone()],
    };
    let proto = save_trigger_rules_request_to_proto(&request);
    assert_eq!(proto.session_id, "sess-1");
    assert_eq!(proto.rules.len(), 1);
    assert!(!proto.rules[0].fire_once);
    assert_eq!(save_trigger_rules_request_from_proto(&proto), request);

    let result = WireSaveTriggerRulesResult { count: 1 };
    let proto = save_trigger_rules_response_to_proto(&result);
    assert_eq!(proto.count, 1);
    assert_eq!(save_trigger_rules_response_from_proto(&proto), result);

    let load = WireLoadTriggerRulesRequest {
        session_id: "sess-1".into(),
    };
    let proto = load_trigger_rules_request_to_proto(&load);
    assert_eq!(proto.session_id, "sess-1");
    assert_eq!(load_trigger_rules_request_from_proto(&proto), load);

    let stored = WireStoredTriggerRule {
        id: "tr-1".into(),
        condition: "file changes".into(),
        action: "run test".into(),
        enabled: true,
        fire_once: false,
        fired_at: None,
        promote_to_chat: true,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let proto = stored_trigger_rule_to_proto(&stored);
    assert!(proto.fired_at.is_none());
    assert_eq!(stored_trigger_rule_from_proto(&proto), stored);

    let loaded = WireLoadTriggerRulesResult {
        rules: vec![stored],
    };
    let proto = load_trigger_rules_response_to_proto(&loaded);
    assert_eq!(proto.rules.len(), 1);
    assert_eq!(load_trigger_rules_response_from_proto(&proto), loaded);
}

#[test]
fn cron_jobs_round_trip_wire_and_proto() {
    let job = WireStoredCronJob {
        id: "cron-1".into(),
        schedule: "*/5 * * * *".into(),
        action: "run backup".into(),
        enabled: true,
        running_trace_id: Some("trace-1".into()),
        last_due_at: Some("2026-01-01T00:00:00Z".into()),
        last_fired_at: Some("2026-01-01T00:05:00Z".into()),
        last_completed_at: Some("2026-01-01T00:05:01Z".into()),
        last_error: None,
        skipped_overlap_count: 3,
        stateful: true,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let request = WireSaveCronJobsRequest {
        session_id: "sess-1".into(),
        jobs: vec![job.clone()],
    };
    let proto = save_cron_jobs_request_to_proto(&request);
    assert_eq!(proto.jobs.len(), 1);
    assert_eq!(proto.jobs[0].skipped_overlap_count, 3);
    assert_eq!(save_cron_jobs_request_from_proto(&proto), request);

    let result = WireSaveCronJobsResult { count: 1 };
    let proto = save_cron_jobs_response_to_proto(&result);
    assert_eq!(proto.count, 1);
    assert_eq!(save_cron_jobs_response_from_proto(&proto), result);

    let load = WireLoadCronJobsRequest {
        session_id: "sess-1".into(),
    };
    let proto = load_cron_jobs_request_to_proto(&load);
    assert_eq!(load_cron_jobs_request_from_proto(&proto), load);

    let stored = WireStoredCronJob {
        id: "cron-1".into(),
        schedule: "*/5 * * * *".into(),
        action: "run backup".into(),
        enabled: true,
        running_trace_id: None,
        last_due_at: None,
        last_fired_at: None,
        last_completed_at: None,
        last_error: Some("failed".into()),
        skipped_overlap_count: 0,
        stateful: false,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let proto = stored_cron_job_to_proto(&stored);
    assert!(proto.last_error.is_some());
    assert_eq!(stored_cron_job_from_proto(&proto), stored);

    let loaded = WireLoadCronJobsResult {
        jobs: vec![stored],
    };
    let proto = load_cron_jobs_response_to_proto(&loaded);
    assert_eq!(proto.jobs.len(), 1);
    assert_eq!(load_cron_jobs_response_from_proto(&proto), loaded);
}
