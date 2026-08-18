//! External tests for `agent::permission` — split out of src
//! (see docs/rust-test-files.md).

use super::super::*;

fn default_policy() -> PermissionPolicy {
    PermissionPolicy::default_for_coding_agent()
}

#[test]
fn control_plane_write_category_allows_by_default() {
    let policy = default_policy();
    match policy.evaluate_with_category(
        PermissionCategory::ControlPlaneWrite,
        "bash",
        &serde_json::json!({"command": "rm -rf /"}),
    ) {
        PermissionDecision::Allow => {}
        other => panic!("control-plane writes must be allowed by default: {other:?}"),
    }
}

#[test]
fn non_bash_tool_names_allow_even_for_custom_policy() {
    let policy = PermissionPolicy::new(
        vec!["shell".into()],
        vec![("danger", r"\bsudo\b")],
    );
    match policy.evaluate("bash", &serde_json::json!({"command": "sudo rm -rf /"})) {
        PermissionDecision::Allow => {}
        other => panic!("bash is not a shell tool for this policy: {other:?}"),
    }
    match policy.evaluate("shell", &serde_json::json!({"command": "sudo ls"})) {
        PermissionDecision::Deny { reason } => assert!(reason.contains("danger")),
        other => panic!("custom shell name must be checked: {other:?}"),
    }
}

#[test]
fn custom_danger_patterns_are_compiled_and_first_match_wins() {
    let policy = PermissionPolicy::new(
        vec!["bash".into()],
        vec![("alpha", r"alpha"), ("beta", r"beta")],
    );
    match policy.evaluate("bash", &serde_json::json!({"command": "beta alpha"})) {
        PermissionDecision::Deny { reason } => assert!(reason.contains("alpha")),
        other => panic!("regex set must match: {other:?}"),
    }
}

#[test]
fn empty_shell_command_fields_allow() {
    let policy = default_policy();
    for args in [
        serde_json::json!({}),
        serde_json::json!({"command": ""}),
        serde_json::json!({"command": "   "}),
        serde_json::json!({"command": 5}),
        serde_json::Value::Null,
    ] {
        match policy.evaluate("bash", &args) {
            PermissionDecision::Allow => {}
            other => panic!("empty/unknown args must allow: {other:?}"),
        }
    }
}

#[test]
fn extract_shell_command_tries_all_fields_and_string_fallback() {
    assert_eq!(
        extract_shell_command(&serde_json::json!({"cmd": "ls"})).as_deref(),
        Some("ls")
    );
    assert_eq!(
        extract_shell_command(&serde_json::json!({"bash": "ls"})).as_deref(),
        Some("ls")
    );
    assert_eq!(
        extract_shell_command(&serde_json::json!({"script": "ls"})).as_deref(),
        Some("ls")
    );
    assert_eq!(
        extract_shell_command(&serde_json::json!("ls -la")).as_deref(),
        Some("ls -la")
    );
    assert_eq!(extract_shell_command(&serde_json::json!("")), None);
}

#[test]
fn split_shell_clauses_splits_on_separators_and_trims_empty_clauses() {
    let clauses = split_shell_clauses("a; b && c || d | e");
    assert_eq!(clauses, vec!["a", "b", "c", "d", "e"]);

    assert!(split_shell_clauses("").is_empty());
    assert_eq!(split_shell_clauses("single"), vec!["single"]);
}

#[test]
fn normalize_operand_strips_one_quote_layer_and_rewrites_brace_home() {
    assert_eq!(normalize_operand("\"/etc\""), "/etc");
    assert_eq!(normalize_operand("'/etc'"), "/etc");
    assert_eq!(normalize_operand("${HOME}/x"), "$HOME/x");
    assert_eq!(normalize_operand("plain"), "plain");
}

#[test]
fn rm_classifier_understands_end_of_options_marker() {
    let policy = default_policy();
    // `rm -- -rf /` should NOT be classified as dangerous: `--` ends flags and
    // `-rf` becomes an operand. If operands include an absolute target it would
    // still be dangerous; `-rf` is not absolute.
    match policy.evaluate("bash", &serde_json::json!({"command": "rm -- -rf /tmp/x"})) {
        PermissionDecision::Deny { .. } => {}
        other => panic!("absolute target must still be caught: {other:?}"),
    }
}

#[test]
fn rm_classifier_requires_both_flags_and_dangerous_target() {
    let policy = default_policy();
    // Bare `-` is treated as an operand (not a flag).
    match policy.evaluate("bash", &serde_json::json!({"command": "rm -r -f -"})) {
        PermissionDecision::Allow => {}
        other => panic!("bare - must be an operand: {other:?}"),
    }
}

#[tokio::test]
async fn as_before_tool_call_blocks_and_allows() {
    let policy = default_policy();
    let hook = policy.as_before_tool_call();

    let ctx = crate::types::BeforeToolCallContext {
        assistant_message: theway_llm_provider::AssistantMessage {
            role: theway_llm_provider::AssistantRole::Assistant,
            content: vec![],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: theway_llm_provider::Usage::default(),
            stop_reason: theway_llm_provider::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        },
        tool_call: theway_llm_provider::ToolCall {
            id: "c1".into(),
            name: "bash".into(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        },
        args: serde_json::json!({"command": "rm -rf /"}),
        context: crate::types::AgentContext::default(),
    };
    let result = hook(
        ctx.clone(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
    assert!(result.block);
    assert!(result.reason.as_deref().unwrap().contains("denied by permission policy"));

    let mut safe_ctx = ctx;
    safe_ctx.args = serde_json::json!({"command": "ls -la"});
    let result = hook(safe_ctx, tokio_util::sync::CancellationToken::new()).await;
    assert!(!result.block);
    assert!(result.reason.is_none());
}
