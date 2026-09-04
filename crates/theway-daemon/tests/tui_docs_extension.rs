#![cfg(feature = "local")]

//! Regression tests for the committed `extensions/tui-docs` package: the
//! plugin reads the TUI documentation from the workspace at load time and
//! registers it as ordered prompt sections appended to `systemInstructions`.

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

fn invocation() -> RuntimeExtensionInvocation {
    let mut context = RuntimeExtensionContext::new("tui-docs-session", "/workspace", 1);
    context.model = Some(ExtensionModelRef {
        provider: "openai".into(),
        model: "target-model".into(),
    });
    RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::BeforeModelRequest,
        ExtensionHookClass::Transform,
        context,
        json!({"request": {
            "provider": "openai", "model": "target-model",
            "systemInstructions": "base", "generationOptions": {}, "tools": [],
        }}),
    )
    .unwrap()
}

/// Single newlines only: no blank lines, no trailing newline, so the
/// section-boundary join ("\n\n") is exactly reproducible.
fn fixture_doc() -> String {
    let mut doc = String::from("TUI_DOC_FIXTURE_START\n");
    let mut line = 0usize;
    // > 16 KiB so the plugin must shard into multiple prompt sections.
    while doc.len() < 17_000 {
        doc.push_str(&format!(
            "行 {line}：theway-tui 客户端与守护进程架构说明文档内容，注入模型上下文。\n"
        ));
        line += 1;
    }
    doc.push_str("TUI_DOC_FIXTURE_END");
    doc
}

#[tokio::test]
async fn injects_tui_document_into_system_instructions_in_order() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_plugin(project.path());
    let doc = fixture_doc();
    std::fs::create_dir_all(project.path().join(".agents").join("overview")).unwrap();
    std::fs::write(project.path().join(".agents").join("overview").join("tui.md"), &doc).unwrap();

    let host = start_host(project.path(), base.path()).await;
    let matched = RuntimeRequestExtensionPort::invoke_request(&*host, invocation())
        .await
        .unwrap();
    let replacement = matched
        .actions
        .iter()
        .find(|action| action.kind == ExtensionActionKind::ReplaceModelRequest)
        .expect("plugin must append a replace_model_request action");
    let instructions = replacement.payload["request"]["systemInstructions"]
        .as_str()
        .unwrap();
    assert!(
        instructions.starts_with("base\n\n"),
        "extension sections must append after the base instructions: {instructions}"
    );
    let appended = &instructions["base\n\n".len()..];
    let parts: Vec<&str> = appended.split("\n\n").collect();
    assert!(
        parts.len() >= 2,
        "a >16 KiB document must shard into multiple sections, got {} part(s)",
        parts.len()
    );
    assert!(
        parts.iter().all(|part| !part.is_empty() && part.len() <= 16_384),
        "every section must stay within the host text limit"
    );
    assert!(
        parts[0].starts_with("TUI_DOC_FIXTURE_START"),
        "first section must open with the document head: {:?}",
        &parts[0][..40]
    );
    assert!(
        parts.last().unwrap().ends_with("TUI_DOC_FIXTURE_END"),
        "last section must close with the document tail"
    );
    // The boundary join inserts one blank line per split; collapsing those
    // reproduces the document exactly.
    assert_eq!(
        appended.replace("\n\n", "\n"),
        doc,
        "injected sections must reproduce the document content losslessly"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn missing_document_registers_nothing_and_leaves_request_untouched() {
    let project = tempdir().unwrap();
    let base = tempdir().unwrap();
    install_plugin(project.path());

    let host = start_host(project.path(), base.path()).await;
    let matched = RuntimeRequestExtensionPort::invoke_request(&*host, invocation())
        .await
        .unwrap();
    assert!(
        matched.actions.is_empty(),
        "no document → no sections → no request actions: {matched:?}"
    );
    host.shutdown().await;
}
