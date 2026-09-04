#![cfg(feature = "local")]

//! Regression tests for the committed `extensions/tui-docs` package: the
//! plugin registers one small prompt-section pointer naming where the TUI
//! documentation lives — it never injects the document body.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use theway_contract::extension::{
    ExtensionActionKind, ExtensionHookClass, ExtensionLifecycleEvent, ExtensionModelRef,
    ExtensionPermission, ExtensionTrustDecision,
};
use theway_core::agent::runtime_extensions::{
    RuntimeExtensionContext, RuntimeExtensionInvocation, RuntimeRequestExtensionPort,
};
use theway_daemon::executor::local::LocalExecutor;
use theway_daemon::ts_extensions::{
    ExtensionBrokerServices, ExtensionTrustStore, PackageCatalog, QuickJsEnginePool,
    SessionPluginHost,
};

fn source_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("extensions")
        .join("tui-docs")
}

fn install_plugin(project: &Path) {
    let target = project.join(".theway").join("extensions").join("tui-docs");
    std::fs::create_dir_all(&target).unwrap();
    for file in ["theway-extension.json", "index.js"] {
        std::fs::copy(source_dir().join(file), target.join(file)).unwrap();
    }
}

fn trust_project(project: &Path, base: &Path) {
    let requested = vec!["workspace.read".parse::<ExtensionPermission>().unwrap()];
    let mut trust = ExtensionTrustStore::load(base);
    trust
        .decide_project(project, requested.clone(), requested, ExtensionTrustDecision::Trusted)
        .unwrap();
    trust.save().unwrap();
}

async fn start_host(project: &Path, base: &Path) -> Arc<SessionPluginHost> {
    trust_project(project, base);
    let services =
        ExtensionBrokerServices::new(base, Arc::new(LocalExecutor::with_cwd(project)));
    let engine = QuickJsEnginePool::with_broker_services(1, Default::default(), services);
    SessionPluginHost::start(
        PackageCatalog::discover(project, base),
        engine,
        "tui-docs-session",
        project,
    )
    .await
}

/// Invoke the before-model-request seam and return the appended instructions.
async fn appended_instructions(host: &Arc<SessionPluginHost>) -> String {
    let mut context = RuntimeExtensionContext::new("tui-docs-session", "/workspace", 1);
    context.model = Some(ExtensionModelRef {
        provider: "openai".into(),
        model: "target-model".into(),
    });
    let invocation = RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
        context,
        json!({"request": {
            "provider": "openai", "model": "target-model",
            "systemInstructions": "base", "generationOptions": {}, "tools": [],
        }}),
    )
    .unwrap();
    let matched = RuntimeRequestExtensionPort::invoke_request(&**host, invocation)
        .await
        .unwrap();
    let replacement = matched
        .actions
        .iter()
        .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
        .expect("plugin must append a replace_model_request action");
    let instructions = replacement.payload["request"]["systemInstructions"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        instructions.starts_with("base\n\n"),
        "pointer must append after the base instructions: {instructions}"
    );
    instructions["base\n\n".len()..].to_string()
}

#[tokio::test]
async fn workspace_document_yields_pointer_not_content() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_plugin(project.path());
    let marker = "TUI_DOC_CONTENT_MARKER_9f3a_never_injected";
    let doc = format!("# TUI doc\n\n{marker}\n\n{}", "x".repeat(16_000));
    std::fs::create_dir_all(project.path().join(".agents").join("overview")).unwrap();
    std::fs::write(project.path().join(".agents").join("overview").join("tui.md"), &doc).unwrap();

    let host = start_host(project.path(), base.path()).await;
    let appended = appended_instructions(&host).await;
    assert!(
        appended.contains(".agents/overview/tui.md"),
        "pointer must name the workspace copy: {appended}"
    );
    assert!(
        !appended.contains(marker),
        "document content must never be injected: {appended}"
    );
    assert!(
        appended.len() < 512,
        "pointer must stay a short sentence, got {} bytes: {appended}",
        appended.len()
    );
    host.shutdown().await;
}

#[tokio::test]
async fn missing_workspace_document_points_at_installed_copy() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_plugin(project.path());

    let host = start_host(project.path(), base.path()).await;
    let appended = appended_instructions(&host).await;
    assert!(
        appended.contains("~/.theway/docs/tui.md"),
        "pointer must name the installed copy: {appended}"
    );
    assert!(
        appended.len() < 512,
        "pointer must stay a short sentence, got {} bytes: {appended}",
        appended.len()
    );
    host.shutdown().await;
}
