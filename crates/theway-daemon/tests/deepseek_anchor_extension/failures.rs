use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::{ExtensionCatalogStatus, ExtensionDiagnosticCode};
use theway_core::agent::runtime_extensions::{
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort,
};
use theway_daemon::ts_extensions::{
    ExtensionReloadDisposition, PackageCatalog, RuntimeExtensionHostConfig,
};

use super::support::*;

#[tokio::test]
async fn bootstrap_unavailable_required_tool_fails_open_without_partial_filter() {
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
    let tools = host
        .merge_registered_tools(Vec::new())
        .iter()
        .map(|tool| tool.definition().clone())
        .collect();

    let result = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();

    assert!(replacement(&result).is_none());
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::HookFailed
            && diagnostic.message.contains("compatible bash")
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn promotion_persistence_failure_keeps_branch_unpromoted() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let state = Arc::new(MemoryStatePort::default());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        state.clone(),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    let tools = merged_tool_definitions(&host, Vec::new());
    RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", tools.clone(), Some(4096)),
    )
    .await
    .unwrap();
    state.set_fail_writes(true);

    RuntimeMessageExtensionPort::invoke_message(
        &*host,
        assistant_invocation(2, "deepseek", "deepseek-chat", "ready"),
    )
    .await
    .unwrap();

    assert!(state.entries().is_empty());
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::HookFailed
            && diagnostic.message.contains("unavailable")
    }));
    state.set_fail_writes(false);
    let retry = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(3, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();
    assert!(replacement(&retry).is_some());
    host.shutdown().await;
}

#[tokio::test]
async fn invalid_configuration_faults_only_anchor_package() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    let mut config = enabled_config();
    config["promotionCondition"]["kind"] = json!("unknown");
    install_anchor(project.path(), &config);

    let (host, engine) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;

    assert!(host.active_extension_ids().await.is_empty());
    assert_eq!(engine.instance_count().await, 0);
    assert!(host.catalog_entries().iter().any(|entry| {
        entry.extension_id == EXTENSION_ID && entry.status == ExtensionCatalogStatus::Faulted
    }));
    assert!(host.diagnostics().iter().any(|diagnostic| {
        diagnostic.code == ExtensionDiagnosticCode::LoadFailed
            && diagnostic.message.contains("promotionCondition.kind")
    }));
    host.shutdown().await;
}

#[tokio::test]
async fn request_timeout_preserves_base_request_and_reports_structured_fault() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, _) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig {
            standard_deadline: Duration::from_nanos(1),
            circuit_failure_threshold: 10,
            ..RuntimeExtensionHostConfig::default()
        },
    )
    .await;
    let tools = merged_tool_definitions(&host, Vec::new());

    let result = RuntimeRequestExtensionPort::invoke_request(
        &*host,
        request_invocation(1, "deepseek", "deepseek-chat", tools, Some(4096)),
    )
    .await
    .unwrap();

    assert!(replacement(&result).is_none());
    assert!(
        host.diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.code == ExtensionDiagnosticCode::HookTimedOut })
    );
    host.shutdown().await;
}

#[tokio::test]
async fn unload_and_reload_reverse_tools_and_rebuild_configuration() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_anchor(project.path(), &enabled_config());
    let (host, engine) = start_anchor(
        project.path(),
        base.path(),
        Arc::new(MemoryStatePort::default()),
        false,
        RuntimeExtensionHostConfig::default(),
    )
    .await;
    assert!(
        host.merge_registered_tools(vec![compatible_bash()])
            .iter()
            .any(|tool| tool.definition().name == "str_replace_editor")
    );

    let mut zero = enabled_config();
    zero["zeroAnchor"] = json!(true);
    std::fs::write(
        package_dir(project.path()).join("anchor-config.json"),
        serde_json::to_vec_pretty(&zero).unwrap(),
    )
    .unwrap();
    let disposition = host
        .request_reload(PackageCatalog::discover(project.path(), base.path()))
        .await
        .unwrap();

    assert!(matches!(
        disposition,
        ExtensionReloadDisposition::Applied { revision: 1 }
    ));
    assert_eq!(host.merge_registered_tools(Vec::new()).len(), 0);
    assert_eq!(engine.instance_count().await, 1);
    host.shutdown().await;
    assert_eq!(host.active_effect_count().await, 0);
    assert_eq!(engine.instance_count().await, 0);
}
