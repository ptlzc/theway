use super::*;

pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        "state.save_dag_run"
            | "storage.save_dag_run"
            | "state.load_dag_runs"
            | "storage.load_dag_runs"
            | "state.save_trigger_rules"
            | "storage.save_trigger_rules"
            | "state.load_trigger_rules"
            | "storage.load_trigger_rules"
            | "state.save_cron_jobs"
            | "storage.save_cron_jobs"
            | "state.load_cron_jobs"
            | "storage.load_cron_jobs"
    )
}

pub(super) async fn dispatch(
    state: &HttpState,
    method: &str,
    params: Option<&serde_json::Value>,
) -> RpcResult {
    match method {
        // ── runtime state storage (issue #84) ───────────────────────────
        "state.save_dag_run" | "storage.save_dag_run" => {
            let request: WireSaveDagRunRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_dag_run(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_dag_runs" | "storage.load_dag_runs" => {
            let request: WireLoadDagRunsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_dag_runs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.save_trigger_rules" | "storage.save_trigger_rules" => {
            let request: WireSaveTriggerRulesRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_trigger_rules(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_trigger_rules" | "storage.load_trigger_rules" => {
            let request: WireLoadTriggerRulesRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_trigger_rules(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.save_cron_jobs" | "storage.save_cron_jobs" => {
            let request: WireSaveCronJobsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .save_cron_jobs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        "state.load_cron_jobs" | "storage.load_cron_jobs" => {
            let request: WireLoadCronJobsRequest = state_params(params)?;
            let result = state
                .storage_ops
                .load_cron_jobs(&request)
                .await
                .map_err(|e| (-32000, e.to_string()))?;
            tool_json(&result)
        }
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}
