//! Runtime state externalization codecs (issue #84): `state.proto`
//! `StorageService` messages ↔ wire models ([`crate::wire`] `Wire*`). The
//! gRPC server converts proto requests into wire requests for the
//! [`crate::transport::StorageOps`] handler and wire results back into proto
//! responses; the [`crate::client::GrpcClient`] storage wrappers run the same
//! codecs in the opposite direction. The JSON-RPC state methods use the wire
//! types directly.

use crate::proto::theway_grpc as proto;
use crate::wire::{
    WireLoadCronJobsRequest, WireLoadCronJobsResult, WireLoadDagRunsRequest, WireLoadDagRunsResult,
    WireLoadTriggerRulesRequest, WireLoadTriggerRulesResult, WireSaveCronJobsRequest,
    WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult, WireStoredCronJob, WireStoredDagRun,
    WireStoredTriggerRule,
};

// ── DAG runs ─────────────────────────────────────────────────────────

pub fn stored_dag_run_to_proto(run: &WireStoredDagRun) -> proto::StoredDagRun {
    proto::StoredDagRun {
        session_id: run.session_id.clone(),
        run_id: run.run_id.clone(),
        snapshot: run.snapshot.clone(),
    }
}

pub fn stored_dag_run_from_proto(run: &proto::StoredDagRun) -> WireStoredDagRun {
    WireStoredDagRun {
        session_id: run.session_id.clone(),
        run_id: run.run_id.clone(),
        snapshot: run.snapshot.clone(),
    }
}

pub fn save_dag_run_request_to_proto(request: &WireSaveDagRunRequest) -> proto::SaveDagRunRequest {
    proto::SaveDagRunRequest {
        session_id: request.session_id.clone(),
        run_id: request.run_id.clone(),
        snapshot: request.snapshot.clone(),
    }
}

pub fn save_dag_run_request_from_proto(
    request: &proto::SaveDagRunRequest,
) -> WireSaveDagRunRequest {
    WireSaveDagRunRequest {
        session_id: request.session_id.clone(),
        run_id: request.run_id.clone(),
        snapshot: request.snapshot.clone(),
    }
}

pub fn save_dag_run_response_to_proto(result: &WireSaveDagRunResult) -> proto::SaveDagRunResponse {
    proto::SaveDagRunResponse {
        saved: result.saved,
    }
}

pub fn save_dag_run_response_from_proto(
    response: &proto::SaveDagRunResponse,
) -> WireSaveDagRunResult {
    WireSaveDagRunResult {
        saved: response.saved,
    }
}

pub fn load_dag_runs_request_to_proto(
    request: &WireLoadDagRunsRequest,
) -> proto::LoadDagRunsRequest {
    proto::LoadDagRunsRequest {
        session_id: request.session_id.clone(),
        run_id: request.run_id.clone(),
    }
}

pub fn load_dag_runs_request_from_proto(
    request: &proto::LoadDagRunsRequest,
) -> WireLoadDagRunsRequest {
    WireLoadDagRunsRequest {
        session_id: request.session_id.clone(),
        run_id: request.run_id.clone(),
    }
}

pub fn load_dag_runs_response_to_proto(
    result: &WireLoadDagRunsResult,
) -> proto::LoadDagRunsResponse {
    proto::LoadDagRunsResponse {
        runs: result.runs.iter().map(stored_dag_run_to_proto).collect(),
    }
}

pub fn load_dag_runs_response_from_proto(
    response: &proto::LoadDagRunsResponse,
) -> WireLoadDagRunsResult {
    WireLoadDagRunsResult {
        runs: response
            .runs
            .iter()
            .map(stored_dag_run_from_proto)
            .collect(),
    }
}

// ── trigger rules ─────────────────────────────────────────────────────

pub fn stored_trigger_rule_to_proto(rule: &WireStoredTriggerRule) -> proto::StoredTriggerRule {
    proto::StoredTriggerRule {
        id: rule.id.clone(),
        condition: rule.condition.clone(),
        action: rule.action.clone(),
        enabled: rule.enabled,
        fire_once: rule.fire_once,
        fired_at: rule.fired_at.clone(),
        promote_to_chat: rule.promote_to_chat,
        created_at: rule.created_at.clone(),
    }
}

pub fn stored_trigger_rule_from_proto(rule: &proto::StoredTriggerRule) -> WireStoredTriggerRule {
    WireStoredTriggerRule {
        id: rule.id.clone(),
        condition: rule.condition.clone(),
        action: rule.action.clone(),
        enabled: rule.enabled,
        fire_once: rule.fire_once,
        fired_at: rule.fired_at.clone(),
        promote_to_chat: rule.promote_to_chat,
        created_at: rule.created_at.clone(),
    }
}

pub fn save_trigger_rules_request_to_proto(
    request: &WireSaveTriggerRulesRequest,
) -> proto::SaveTriggerRulesRequest {
    proto::SaveTriggerRulesRequest {
        session_id: request.session_id.clone(),
        rules: request
            .rules
            .iter()
            .map(stored_trigger_rule_to_proto)
            .collect(),
    }
}

pub fn save_trigger_rules_request_from_proto(
    request: &proto::SaveTriggerRulesRequest,
) -> WireSaveTriggerRulesRequest {
    WireSaveTriggerRulesRequest {
        session_id: request.session_id.clone(),
        rules: request
            .rules
            .iter()
            .map(stored_trigger_rule_from_proto)
            .collect(),
    }
}

pub fn save_trigger_rules_response_to_proto(
    result: &WireSaveTriggerRulesResult,
) -> proto::SaveTriggerRulesResponse {
    proto::SaveTriggerRulesResponse {
        count: result.count,
    }
}

pub fn save_trigger_rules_response_from_proto(
    response: &proto::SaveTriggerRulesResponse,
) -> WireSaveTriggerRulesResult {
    WireSaveTriggerRulesResult {
        count: response.count,
    }
}

pub fn load_trigger_rules_request_to_proto(
    request: &WireLoadTriggerRulesRequest,
) -> proto::LoadTriggerRulesRequest {
    proto::LoadTriggerRulesRequest {
        session_id: request.session_id.clone(),
    }
}

pub fn load_trigger_rules_request_from_proto(
    request: &proto::LoadTriggerRulesRequest,
) -> WireLoadTriggerRulesRequest {
    WireLoadTriggerRulesRequest {
        session_id: request.session_id.clone(),
    }
}

pub fn load_trigger_rules_response_to_proto(
    result: &WireLoadTriggerRulesResult,
) -> proto::LoadTriggerRulesResponse {
    proto::LoadTriggerRulesResponse {
        rules: result
            .rules
            .iter()
            .map(stored_trigger_rule_to_proto)
            .collect(),
    }
}

pub fn load_trigger_rules_response_from_proto(
    response: &proto::LoadTriggerRulesResponse,
) -> WireLoadTriggerRulesResult {
    WireLoadTriggerRulesResult {
        rules: response
            .rules
            .iter()
            .map(stored_trigger_rule_from_proto)
            .collect(),
    }
}

// ── cron jobs ─────────────────────────────────────────────────────────

pub fn stored_cron_job_to_proto(job: &WireStoredCronJob) -> proto::StoredCronJob {
    proto::StoredCronJob {
        id: job.id.clone(),
        schedule: job.schedule.clone(),
        action: job.action.clone(),
        enabled: job.enabled,
        running_trace_id: job.running_trace_id.clone(),
        last_due_at: job.last_due_at.clone(),
        last_fired_at: job.last_fired_at.clone(),
        last_completed_at: job.last_completed_at.clone(),
        last_error: job.last_error.clone(),
        skipped_overlap_count: job.skipped_overlap_count,
        stateful: job.stateful,
        created_at: job.created_at.clone(),
    }
}

pub fn stored_cron_job_from_proto(job: &proto::StoredCronJob) -> WireStoredCronJob {
    WireStoredCronJob {
        id: job.id.clone(),
        schedule: job.schedule.clone(),
        action: job.action.clone(),
        enabled: job.enabled,
        running_trace_id: job.running_trace_id.clone(),
        last_due_at: job.last_due_at.clone(),
        last_fired_at: job.last_fired_at.clone(),
        last_completed_at: job.last_completed_at.clone(),
        last_error: job.last_error.clone(),
        skipped_overlap_count: job.skipped_overlap_count,
        stateful: job.stateful,
        created_at: job.created_at.clone(),
    }
}

pub fn save_cron_jobs_request_to_proto(
    request: &WireSaveCronJobsRequest,
) -> proto::SaveCronJobsRequest {
    proto::SaveCronJobsRequest {
        session_id: request.session_id.clone(),
        jobs: request.jobs.iter().map(stored_cron_job_to_proto).collect(),
    }
}

pub fn save_cron_jobs_request_from_proto(
    request: &proto::SaveCronJobsRequest,
) -> WireSaveCronJobsRequest {
    WireSaveCronJobsRequest {
        session_id: request.session_id.clone(),
        jobs: request
            .jobs
            .iter()
            .map(stored_cron_job_from_proto)
            .collect(),
    }
}

pub fn save_cron_jobs_response_to_proto(
    result: &WireSaveCronJobsResult,
) -> proto::SaveCronJobsResponse {
    proto::SaveCronJobsResponse {
        count: result.count,
    }
}

pub fn save_cron_jobs_response_from_proto(
    response: &proto::SaveCronJobsResponse,
) -> WireSaveCronJobsResult {
    WireSaveCronJobsResult {
        count: response.count,
    }
}

pub fn load_cron_jobs_request_to_proto(
    request: &WireLoadCronJobsRequest,
) -> proto::LoadCronJobsRequest {
    proto::LoadCronJobsRequest {
        session_id: request.session_id.clone(),
    }
}

pub fn load_cron_jobs_request_from_proto(
    request: &proto::LoadCronJobsRequest,
) -> WireLoadCronJobsRequest {
    WireLoadCronJobsRequest {
        session_id: request.session_id.clone(),
    }
}

pub fn load_cron_jobs_response_to_proto(
    result: &WireLoadCronJobsResult,
) -> proto::LoadCronJobsResponse {
    proto::LoadCronJobsResponse {
        jobs: result.jobs.iter().map(stored_cron_job_to_proto).collect(),
    }
}

pub fn load_cron_jobs_response_from_proto(
    response: &proto::LoadCronJobsResponse,
) -> WireLoadCronJobsResult {
    WireLoadCronJobsResult {
        jobs: response
            .jobs
            .iter()
            .map(stored_cron_job_from_proto)
            .collect(),
    }
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("state");
