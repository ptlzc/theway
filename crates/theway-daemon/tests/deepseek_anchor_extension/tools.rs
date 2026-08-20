use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::ExtensionDiagnosticCode;
use theway_core::agent::runtime_extensions::RuntimeRequestExtensionPort;
use theway_daemon::ts_extensions::RuntimeExtensionHostConfig;
use tokio_util::sync::CancellationToken;

use super::support::*;

#[tokio::test]
async fn tool_registration_selects_compatible_base_tools_without_override() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;

    let merged = host.merge_registered_tools(vec![compatible_bash(), compatible_editor()]);

    assert_eq!(merged.len(), 2);
    let editor = merged
        .iter()
        .find(|tool| tool.definition().name == "str_replace_editor")
        .unwrap();
    assert_eq!(
        editor.definition().description,
        "str_replace_editor test tool"
    );
    let definitions = merged
        .iter()
        .map(|tool| tool.definition().clone())
        .collect();
    let result = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", definitions, Some(4096)),
    )
    .await
    .unwrap();
    assert!(replacement(&result).is_some());
    host.shutdown().await;
}

#[tokio::test]
async fn str_replace_editor_uses_workspace_broker_for_all_supported_commands() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let tools = host.merge_registered_tools(vec![compatible_bash()]);
    let editor = tools
        .iter()
        .find(|tool| tool.definition().name == "str_replace_editor")
        .unwrap();
    let cancel = CancellationToken::new();

    editor
        .execute(
            "create",
            json!({"command": "create", "path": "sample.txt", "file_text": "alpha\nbeta"}),
            cancel.clone(),
            None,
        )
        .await
        .unwrap();
    let viewed = editor
        .execute(
            "view",
            json!({"command": "view", "path": "sample.txt", "view_range": [2, -1]}),
            cancel.clone(),
            None,
        )
        .await
        .unwrap();
    let viewed = serde_json::to_value(viewed).unwrap();
    assert!(
        viewed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("2  beta")
    );
    editor
        .execute(
            "replace",
            json!({"command": "str_replace", "path": "sample.txt", "old_str": "beta", "new_str": "gamma"}),
            cancel.clone(),
            None,
        )
        .await
        .unwrap();
    editor
        .execute(
            "insert",
            json!({"command": "insert", "path": "sample.txt", "insert_line": 1, "new_str": "between"}),
            cancel,
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(project.path().join("sample.txt")).unwrap(),
        "alpha\nbetween\ngamma"
    );
    assert!(host.audit_events().iter().any(|event| {
        event.operation == theway_contract::extension::ExtensionAuditOperation::WorkspaceWrite
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn incompatible_editor_requires_explicit_override_permission() {
    let denied_project = tempdir().unwrap();
    let denied_base = tempdir().unwrap();
    install_anchor(denied_project.path(), &enabled_config());
    let (denied, _) = start_anchor(
        denied_project.path(),
        denied_base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let denied_tools =
        denied.merge_registered_tools(vec![compatible_bash(), incompatible_editor()]);
    let denied_result = RuntimeRequestExtensionPort::invoke_request(
        &*denied,
        request_invocation(
            1,
            "deepseek",
            "deepseek-chat",
            denied_tools
                .iter()
                .map(|tool| tool.definition().clone())
                .collect(),
            Some(4096),
        ),
    )
    .await
    .unwrap();
    assert!(replacement(&denied_result).is_none());
    assert!(denied.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::HookFailed
            && diagnostic.message.contains("anchor_configuration")
    }));
    denied.shutdown().await;

    let granted_project = tempdir().unwrap();
    let granted_base = tempdir().unwrap();
    install_anchor(granted_project.path(), &enabled_config());
    let (granted, _) = start_anchor(
        granted_project.path(),
        granted_base.path(),
        Arc::new(MemoryStatePort::default()),
        true,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let granted_tools =
        granted.merge_registered_tools(vec![compatible_bash(), incompatible_editor()]);
    let editor = granted_tools
        .iter()
        .find(|tool| tool.definition().name == "str_replace_editor")
        .unwrap();
    assert!(
        editor.definition().parameters["properties"]["command"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "str_replace")
    );
    let granted_result = RuntimeRequestExtensionPort::invoke_request(
        &*granted,
        request_invocation(
            1,
            "deepseek",
            "deepseek-chat",
            granted_tools
                .iter()
                .map(|tool| tool.definition().clone())
                .collect(),
            Some(4096),
        ),
    )
    .await
    .unwrap();
    assert!(replacement(&granted_result).is_some());
    granted.shutdown().await;
}
