use std::sync::Arc;

use serde_json::json;
use theway_contract::extension::{ExtensionHookClass, ExtensionLifecycleEvent};
use theway_core::agent::runtime_extensions::{
    NoopSessionExtensionStatePort, RuntimeCompactionExtensionPort, RuntimeMessageExtensionPort,
    RuntimeRequestExtensionPort, RuntimeRunExtensionPort, RuntimeSessionExtensionPort,
    RuntimeToolExtensionPort, RuntimeExtensionInvocation,
};

use super::super::catalog::PackageCatalog;
use super::super::engine::QuickJsEnginePool;
use super::super::host::SessionPluginHost;
use super::super::dispatcher::RuntimeExtensionHostConfig;

async fn empty_host() -> std::sync::Arc<SessionPluginHost> {
    let cwd = tempfile::tempdir().unwrap();
    SessionPluginHost::load_with_state(
        PackageCatalog::default(),
        QuickJsEnginePool::new(1),
        "sess",
        cwd.path(),
        RuntimeExtensionHostConfig::default(),
        Arc::new(NoopSessionExtensionStatePort),
    )
    .await
}

fn invocation(event: ExtensionLifecycleEvent, class: ExtensionHookClass) -> RuntimeExtensionInvocation {
    RuntimeExtensionInvocation::new(
        event,
        class,
        theway_core::agent::runtime_extensions::RuntimeExtensionContext::new("sess", "/cwd", 1),
        json!({}),
    )
    .unwrap()
}

#[tokio::test]
async fn runtime_ports_delegate_to_empty_host() {
    let host = empty_host().await;
    let input = invocation(ExtensionLifecycleEvent::Input, ExtensionHookClass::Transform);
    let session = invocation(ExtensionLifecycleEvent::SessionStart, ExtensionHookClass::Observe);
    let run = invocation(ExtensionLifecycleEvent::RunStarted, ExtensionHookClass::Observe);
    let tool = invocation(ExtensionLifecycleEvent::ToolExecutionStart, ExtensionHookClass::Observe);
    let request = invocation(ExtensionLifecycleEvent::BeforeModelRequest, ExtensionHookClass::Transform);

    assert!(host.invoke_session(session).await.unwrap().actions.is_empty());
    assert!(host.invoke_message(input.clone()).await.unwrap().actions.is_empty());
    assert!(host.invoke_compaction(input.clone()).await.unwrap().actions.is_empty());
    assert!(host.invoke_run(run).await.unwrap().actions.is_empty());
    assert!(host.invoke_tool(tool).await.unwrap().actions.is_empty());
    assert!(host.invoke_request(request).await.unwrap().actions.is_empty());
    assert!(!host.has_request_hook(ExtensionLifecycleEvent::BeforeModelRequest, ExtensionHookClass::Transform));
}
