//! Tests for `triggers::cron::tools` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use theway_llm_provider::UserContentBlock;

use crate::triggers::cron::CronJob;

async fn execute(
    tool: &dyn AgentTool,
    params: Value,
) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("call-1", params, CancellationToken::new(), None)
        .await
}

fn unique_action(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("cron-tools-test-{tag}-{}-{}", std::process::id(), n)
}

fn block_text(block: &UserContentBlock) -> String {
    match block {
        UserContentBlock::Text(text) => text.text.clone(),
        UserContentBlock::Image(_) => String::new(),
    }
}

fn add_job(tag: &str) -> CronJob {
    let action = unique_action(tag);
    crate::triggers::global_cron_registry()
        .add_job_full("*/10 * * * *", &action, false)
        .unwrap_or_else(|e| panic!("add_job_full failed: {e}"))
}

#[test]
fn tool_definitions_labels_and_modes_are_stable() {
    let new = NewCronJobTool::new(None, crate::triggers::global_cron_registry().clone());
    let list = ListCronJobsTool::new(crate::triggers::global_cron_registry().clone());
    let remove = RemoveCronJobTool::new(None, crate::triggers::global_cron_registry().clone());
    let state = SetCronJobStateTool::new(None, crate::triggers::global_cron_registry().clone());

    assert_eq!(new.definition().name, "new_cron_job");
    assert_eq!(new.label(), "new_cron_job");
    assert_eq!(list.definition().name, "list_cron_jobs");
    assert_eq!(list.label(), "list_cron_jobs");
    assert_eq!(remove.definition().name, "remove_cron_job");
    assert_eq!(remove.label(), "remove_cron_job");
    assert_eq!(state.definition().name, "set_cron_job_state");
    assert_eq!(state.label(), "set_cron_job_state");

    assert!(matches!(new.execution_mode(), Some(ToolExecutionMode::Sequential)));
    assert!(matches!(list.execution_mode(), Some(ToolExecutionMode::Parallel)));
    assert!(matches!(remove.execution_mode(), Some(ToolExecutionMode::Sequential)));
    assert!(matches!(state.execution_mode(), Some(ToolExecutionMode::Sequential)));
}

#[tokio::test]
async fn new_cron_job_execute_validates_required_args_and_schedule() {
    let tool = NewCronJobTool::new(None, crate::triggers::global_cron_registry().clone());

    let missing_schedule = execute(&tool, json!({}))
        .await
        .expect_err("missing schedule must fail");
    assert!(
        missing_schedule.to_string().contains("missing required arg: schedule"),
        "got: {missing_schedule}"
    );

    let missing_action = execute(&tool, json!({ "schedule": "* * * * *" }))
        .await
        .expect_err("missing action must fail");
    assert!(
        missing_action.to_string().contains("missing required arg: action"),
        "got: {missing_action}"
    );

    let bad_schedule = execute(
        &tool,
        json!({ "schedule": "not-a-schedule", "action": "echo hi" }),
    )
    .await
    .expect_err("bad schedule must fail");
    assert!(
        bad_schedule.to_string().contains("invalid schedule"),
        "got: {bad_schedule}"
    );
}

#[tokio::test]
async fn new_cron_job_execute_creates_session_scoped_job() {
    let tool = NewCronJobTool::new(None, crate::triggers::global_cron_registry().clone());
    let action = unique_action("create");

    let result = execute(
        &tool,
        json!({
            "schedule": "*/10 * * * *",
            "action": action,
            "stateful": true
        }),
    )
    .await
    .expect("create should succeed");

    let job = crate::triggers::global_cron_registry()
        .list()
        .into_iter()
        .find(|job| job.action == action)
        .expect("created job must be in registry");
    assert_eq!(job.schedule, "*/10 * * * *");
    assert!(job.stateful);
    assert_eq!(result.details["id"], job.id);
    assert_eq!(result.details["schedule"], "*/10 * * * *");
    assert_eq!(result.details["action"], action);
    assert_eq!(result.details["enabled"], true);
    assert_eq!(result.details["stateful"], true);
    assert_eq!(result.details["scope"], "session");
    assert!(result.details["audit_entry_id"].is_null());
    assert!(
        block_text(&result.content[0]).contains("created cron job"),
        "{:?}",
        result.content
    );

    crate::triggers::global_cron_registry().remove_job(&job.id).unwrap();
}

#[tokio::test]
async fn list_cron_jobs_execute_renders_registry() {
    let job = add_job("list");

    let result = execute(&ListCronJobsTool::new(crate::triggers::global_cron_registry().clone()), json!({}))
        .await
        .expect("list should succeed");

    // The registry is process-global and other tests may run concurrently;
    // only assert properties that are stable for this test's own job.
    assert!(result.details["count"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(result.details["scope"], "session");
    let content = block_text(&result.content[0]);
    assert!(content.contains("session cron jobs:"), "{content}");
    assert!(content.contains(&job.id), "{content}");

    crate::triggers::global_cron_registry().remove_job(&job.id).unwrap();
}

#[tokio::test]
async fn remove_cron_job_execute_validates_id_and_confirmation() {
    let tool = RemoveCronJobTool::new(None, crate::triggers::global_cron_registry().clone());

    let missing = execute(&tool, json!({}))
        .await
        .expect_err("missing id must fail");
    assert!(
        missing.to_string().contains("missing required arg: id"),
        "got: {missing}"
    );

    let unknown = execute(&tool, json!({ "id": "cron-does-not-exist" }))
        .await
        .expect_err("unknown id must fail");
    assert!(
        unknown.to_string().contains("no cron job with id 'cron-does-not-exist'"),
        "got: {unknown}"
    );

    let job = add_job("remove-confirm");
    let preview = execute(&tool, json!({ "id": job.id }))
        .await
        .expect("preview should succeed");
    assert!(
        block_text(&preview.content[0]).contains("requires confirmation"),
        "{:?}",
        preview.content
    );
    assert_eq!(preview.details["confirmation_required"], true);
    assert_eq!(preview.details["removed_count"], 0);

    let removed = execute(&tool, json!({ "id": job.id, "confirm": true }))
        .await
        .expect("confirmed removal should succeed");
    assert_eq!(removed.details["removed_count"], 1);
    assert_eq!(removed.details["id"], job.id);
    assert!(
        crate::triggers::global_cron_registry()
            .list()
            .iter()
            .all(|existing| existing.id != job.id),
        "job should be removed"
    );
}

#[tokio::test]
async fn set_cron_job_state_execute_validates_args_and_refuses_enable() {
    let tool = SetCronJobStateTool::new(None, crate::triggers::global_cron_registry().clone());

    let missing_id = execute(&tool, json!({ "enabled": false }))
        .await
        .expect_err("missing id must fail");
    assert!(
        missing_id.to_string().contains("missing required arg: id"),
        "got: {missing_id}"
    );

    let missing_enabled = execute(&tool, json!({ "id": "cron-1" }))
        .await
        .expect_err("missing enabled must fail");
    assert!(
        missing_enabled.to_string().contains("missing required arg: enabled"),
        "got: {missing_enabled}"
    );

    let enable = execute(&tool, json!({ "id": "cron-1", "enabled": true }))
        .await
        .expect_err("model-facing enable must fail");
    assert!(
        enable.to_string().contains("requires user confirmation"),
        "got: {enable}"
    );

    let unknown = execute(
        &tool,
        json!({ "id": "cron-does-not-exist", "enabled": false }),
    )
    .await
    .expect_err("unknown id must fail");
    assert!(
        unknown.to_string().contains("no cron job with id 'cron-does-not-exist'"),
        "got: {unknown}"
    );
}

#[tokio::test]
async fn set_cron_job_state_execute_disables_job() {
    let tool = SetCronJobStateTool::new(None, crate::triggers::global_cron_registry().clone());
    let job = add_job("disable");

    let result = execute(&tool, json!({ "id": job.id, "enabled": false }))
        .await
        .expect("disable should succeed");

    assert_eq!(result.details["id"], job.id);
    assert_eq!(result.details["enabled"], false);
    assert_eq!(result.details["scope"], "session");
    assert!(
        block_text(&result.content[0]).contains("state: disabled"),
        "{:?}",
        result.content
    );

    let jobs = crate::triggers::global_cron_registry().list();
    let updated = jobs.iter().find(|j| j.id == job.id).expect("job exists");
    assert!(!updated.enabled);
    assert!(updated.running_trace_id.is_none());

    crate::triggers::global_cron_registry().remove_job(&job.id).unwrap();
}
