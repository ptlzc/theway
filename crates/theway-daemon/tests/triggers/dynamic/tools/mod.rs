//! Tests for `triggers::dynamic::tools` — split out of src (see docs/rust-test-files.md).

use super::*;
use chrono::Utc;

async fn execute(
    tool: &dyn AgentTool,
    params: Value,
) -> Result<AgentToolResult, AgentToolError> {
    tool.execute("call-1", params, CancellationToken::new(), None)
        .await
}

fn sample_rule() -> DynamicTriggerRule {
    DynamicTriggerRule {
        id: "dyn-sample".into(),
        condition: "a build finishes".into(),
        action: "run cargo test".into(),
        enabled: true,
        fire_once: true,
        fired_at: None,
        promote_to_chat: false,
        created_at: Utc::now(),
    }
}

#[test]
fn definitions_and_labels_are_catalog_names() {
    let new = NewTriggerTool::new(crate::triggers::global_registry().clone());
    let list = ListTriggersTool::new(crate::triggers::global_registry().clone());
    let remove = RemoveTriggerTool::new(crate::triggers::global_registry().clone());
    let state = SetTriggerStateTool::new(crate::triggers::global_registry().clone());

    assert_eq!(new.definition().name, "new_trigger");
    assert_eq!(new.label(), "new_trigger");
    assert_eq!(list.definition().name, "list_triggers");
    assert_eq!(list.label(), "list_triggers");
    assert_eq!(remove.definition().name, "remove_trigger");
    assert_eq!(remove.label(), "remove_trigger");
    assert_eq!(state.definition().name, "set_trigger_state");
    assert_eq!(state.label(), "set_trigger_state");
    assert!(matches!(new.execution_mode(), Some(ToolExecutionMode::Parallel)));
    assert!(matches!(list.execution_mode(), Some(ToolExecutionMode::Parallel)));
    assert!(matches!(remove.execution_mode(), Some(ToolExecutionMode::Parallel)));
    assert!(matches!(state.execution_mode(), Some(ToolExecutionMode::Parallel)));
}

#[test]
fn render_trigger_rules_for_tool_empty() {
    assert_eq!(render_trigger_rules_for_tool(&[]), "dynamic trigger rules: none");
}

#[test]
fn render_trigger_rules_for_tool_formats_rule_modes() {
    let rule = sample_rule();
    let rendered = render_trigger_rules_for_tool(std::slice::from_ref(&rule));

    assert!(rendered.contains("dynamic trigger rules: 1"), "{rendered}");
    assert!(rendered.contains("dyn-sample"), "{rendered}");
    assert!(rendered.contains("enabled"), "{rendered}");
    assert!(rendered.contains("fire_once"), "{rendered}");
    assert!(rendered.contains("audit_only"), "{rendered}");
    assert!(rendered.contains("a build finishes"), "{rendered}");
    assert!(rendered.contains("run cargo test"), "{rendered}");
    assert!(rendered.contains(&rule.created_at.to_rfc3339()), "{rendered}");
}

#[test]
fn render_trigger_rules_for_tool_shows_disabled_repeat_and_promoted_modes() {
    let rule = DynamicTriggerRule {
        enabled: false,
        fire_once: false,
        promote_to_chat: true,
        ..sample_rule()
    };
    let rendered = render_trigger_rules_for_tool(std::slice::from_ref(&rule));

    assert!(rendered.contains("disabled"), "{rendered}");
    assert!(rendered.contains("repeat"), "{rendered}");
    assert!(rendered.contains("promote_to_chat"), "{rendered}");
}

#[test]
fn looks_like_fixed_schedule_request_detects_english_and_chinese() {
    for text in [
        "Every hour",
        "check hourly",
        "Every day at noon",
        "daily report",
        "Every week on monday",
        "weekly digest",
        "scheduled job please",
        "run this cron",
        "crontab entry",
        "每小时",
        "每小時",
        "每天",
        "每日",
        "每周",
        "每週",
        "定时任务",
        "定時任務",
    ] {
        assert!(
            looks_like_fixed_schedule_request(text),
            "should look like a fixed schedule request: {text}"
        );
    }

    for text in [
        "when a build finishes",
        "if a new issue is created",
        "on any future event matching this condition",
    ] {
        assert!(
            !looks_like_fixed_schedule_request(text),
            "should not look like a fixed schedule request: {text}"
        );
    }
}

#[test]
fn new_trigger_permission_reason_is_shape_only() {
    let cls = NewTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(
        &json!({ "condition": "event with secret", "action": "echo secret" }),
    );
    match cls {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("`condition` + `action`"), "{reason}");
            assert!(!reason.contains("secret"), "{reason}");
        }
        other => panic!("must prompt, got {other:?}"),
    }

    let spec_cls = NewTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({ "spec": "spec text" }));
    match spec_cls {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("`spec` field"), "{reason}");
        }
        other => panic!("must prompt, got {other:?}"),
    }

    let fallback = NewTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({}));
    match fallback {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("create dynamic trigger"), "{reason}");
        }
        other => panic!("must prompt, got {other:?}"),
    }
}

#[test]
fn remove_trigger_permission_reason_distinguishes_single_and_all() {
    let single = RemoveTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({ "id": "dyn-1" }));
    match single {
        PermissionClassification::Prompt { reason } => assert!(reason.contains("`dyn-1`"), "{reason}"),
        other => panic!("must prompt, got {other:?}"),
    }

    let all = RemoveTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({ "all": true }));
    match all {
        PermissionClassification::Prompt { reason } => assert!(reason.contains("ALL"), "{reason}"),
        other => panic!("must prompt, got {other:?}"),
    }

    let fallback = RemoveTriggerTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({}));
    match fallback {
        PermissionClassification::Prompt { reason } => assert!(reason.contains("remove dynamic trigger"), "{reason}"),
        other => panic!("must prompt, got {other:?}"),
    }
}

#[test]
fn set_trigger_state_permission_allows_disable_and_prompts_enable() {
    let disable = SetTriggerStateTool::new(crate::triggers::global_registry().clone()).permission_classification(&json!({ "enabled": false }));
    assert!(matches!(disable, PermissionClassification::Allow));

    let enable = SetTriggerStateTool::new(crate::triggers::global_registry().clone()).permission_classification(
        &json!({ "id": "dyn-9", "enabled": true }),
    );
    match enable {
        PermissionClassification::Prompt { reason } => {
            assert!(reason.contains("dyn-9"), "{reason}");
        }
        other => panic!("enable must prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn new_trigger_execute_routes_fixed_schedule_to_cron() {
    let tool = NewTriggerTool::new(crate::triggers::global_registry().clone());

    for params in [
        json!({ "condition": "Every hour", "action": "echo hi" }),
        json!({ "condition": "event happens", "action": "每天" }),
        json!({ "condition": "event happens", "action": "echo hi", "spec": "每周" }),
    ] {
        let err = execute(&tool, params)
            .await
            .expect_err("fixed schedule must be routed to cron");
        assert!(
            err.to_string().contains("new_cron_job"),
            "got: {err}"
        );
    }
}

#[tokio::test]
async fn new_trigger_execute_missing_args_errors() {
    let tool = NewTriggerTool::new(crate::triggers::global_registry().clone());
    let err = execute(&tool, json!({}))
        .await
        .expect_err("missing args must fail");
    assert!(
        err.to_string().contains("missing required args"),
        "got: {err}"
    );
}

#[tokio::test]
async fn remove_trigger_execute_missing_id_errors() {
    let tool = RemoveTriggerTool::new(crate::triggers::global_registry().clone());
    let err = execute(&tool, json!({}))
        .await
        .expect_err("missing id must fail");
    assert!(
        err.to_string().contains("missing required arg: id"),
        "got: {err}"
    );
}

#[tokio::test]
async fn remove_trigger_execute_unknown_id_errors() {
    let tool = RemoveTriggerTool::new(crate::triggers::global_registry().clone());
    let err = execute(&tool, json!({ "id": "dyn-does-not-exist" }))
        .await
        .expect_err("unknown id must fail");
    assert!(
        err.to_string().contains("no dynamic trigger rule with id"),
        "got: {err}"
    );
}

#[tokio::test]
async fn set_trigger_state_execute_missing_id_or_enabled_errors() {
    let tool = SetTriggerStateTool::new(crate::triggers::global_registry().clone());

    let missing_id = execute(&tool, json!({ "enabled": false }))
        .await
        .expect_err("missing id must fail");
    assert!(
        missing_id.to_string().contains("missing required arg: id"),
        "got: {missing_id}"
    );

    let missing_enabled = execute(&tool, json!({ "id": "dyn-1" }))
        .await
        .expect_err("missing enabled must fail");
    assert!(
        missing_enabled.to_string().contains("missing required arg: enabled"),
        "got: {missing_enabled}"
    );
}

#[tokio::test]
async fn set_trigger_state_execute_unknown_id_errors() {
    let tool = SetTriggerStateTool::new(crate::triggers::global_registry().clone());
    let err = execute(
        &tool,
        json!({ "id": "dyn-does-not-exist", "enabled": true }),
    )
    .await
    .expect_err("unknown id must fail");
    assert!(
        err.to_string().contains("no dynamic trigger rule with id"),
        "got: {err}"
    );
}
