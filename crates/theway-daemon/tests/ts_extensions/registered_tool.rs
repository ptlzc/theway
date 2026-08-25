use serde_json::json;
use theway_core::{AgentTool, PermissionClassification};
use tokio_util::sync::CancellationToken;

use super::super::engine::{EngineInstanceKey, QuickJsEnginePool};
use super::super::registered_tool::RegisteredExtensionTool;
use super::super::registration_runtime::RegistrationRuntime;
use super::super::registrations::{ToolPermission, ToolRegistration};

fn tool(permission: ToolPermission) -> RegisteredExtensionTool {
    let registration = ToolRegistration {
        definition: theway_llm_provider::Tool {
            name: "ext_tool".into(),
            description: "desc".into(),
            parameters: json!({"type": "object"}),
        },
        label: "Extension Tool".into(),
        result_schema: None,
        permission,
        scope: theway_contract::extension::ExtensionScope::Session,
        override_existing: false,
    };
    RegisteredExtensionTool::new(
        &registration,
        1,
        EngineInstanceKey::new("sess", "ext"),
        "/cwd".into(),
        QuickJsEnginePool::new(1),
        RegistrationRuntime::default(),
    )
}

#[test]
fn definition_and_label_match_registration() {
    let ext_tool = tool(ToolPermission::Allow);
    assert_eq!(ext_tool.definition().name, "ext_tool");
    assert_eq!(ext_tool.label(), "Extension Tool");
}

#[test]
fn permission_classification_maps_all_modes() {
    let allow = tool(ToolPermission::Allow);
    assert!(matches!(
        allow.permission_classification(&json!({})),
        PermissionClassification::Allow
    ));

    let prompt = tool(ToolPermission::Prompt);
    match prompt.permission_classification(&json!({})) {
        PermissionClassification::Prompt { reason } => assert!(reason.contains("approval")),
        other => panic!("expected prompt, got {other:?}"),
    }

    let block = tool(ToolPermission::Block);
    match block.permission_classification(&json!({})) {
        PermissionClassification::Block { reason } => assert!(reason.contains("blocked")),
        other => panic!("expected block, got {other:?}"),
    }
}

#[tokio::test]
async fn execute_rejects_disposed_registration_without_engine_call() {
    let ext_tool = tool(ToolPermission::Allow);
    let err = ext_tool
        .execute("call-1", json!({}), CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("disposed"), "{err}");
}
