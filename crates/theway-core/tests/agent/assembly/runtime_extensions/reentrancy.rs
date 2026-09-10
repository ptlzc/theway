//! Reentrancy guard and gate behaviour: nested dispatch rejection, model and
//! session-switch gates, shutdown settlement, and before-run persist failure.

use async_trait::async_trait;

use super::*;

#[derive(Default)]
struct ReentrantPort {
    harness: OnceLock<Arc<AgentHarness>>,
    nested_error: Mutex<Option<String>>,
}

#[async_trait]
impl RuntimeRequestExtensionPort for ReentrantPort {
    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            let error = self
                .harness
                .get()
                .unwrap()
                .prompt("nested")
                .await
                .unwrap_err();
            *self.nested_error.lock() = Some(error.to_string());
        }
        Ok(empty_batch())
    }
}

macro_rules! impl_reentrant_noop {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for ReentrantPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                Ok(empty_batch())
            }
        }
    };
}

impl_reentrant_noop!(RuntimeSessionExtensionPort, invoke_session);
impl_reentrant_noop!(RuntimeRunExtensionPort, invoke_run);
impl_reentrant_noop!(RuntimeMessageExtensionPort, invoke_message);
impl_reentrant_noop!(RuntimeToolExtensionPort, invoke_tool);
impl_reentrant_noop!(RuntimeCompactionExtensionPort, invoke_compaction);

#[tokio::test]
async fn nested_user_send_from_lifecycle_dispatch_is_rejected_without_nested_provider_run() {
    let port = Arc::new(ReentrantPort::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut options = AgentHarnessOptions::new(
        faux_model(),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );
    options.runtime_extensions = port.clone();
    options.stream_fn = Some(success_stream(calls.clone()));
    let harness = Arc::new(AgentHarness::new(options));
    port.harness.set(harness.clone()).unwrap_or_else(|_| unreachable!());

    harness.prompt("outer").await.unwrap();

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(
        port.nested_error
            .lock()
            .as_deref()
            .unwrap()
            .contains("cannot be started synchronously")
    );
}

#[tokio::test]
async fn model_and_branch_gates_cancel_before_persisted_or_active_state_changes() {
    let port = Arc::new(RecordingPort::default());
    for event in [
        ExtensionLifecycleEvent::BeforeModelSelection,
        ExtensionLifecycleEvent::BeforeSessionSwitch,
    ] {
        port.respond(
            event,
            ExtensionHookClass::Gate,
            ExtensionActionBatch {
                decision: Some(ExtensionGateDecision::Cancel {
                    code: "test.cancelled".into(),
                    message: "cancelled by test".into(),
                }),
                actions: Vec::new(),
            },
        );
    }
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let root = session.append_message(user_message("root")).await.unwrap();
    let harness = harness_with_port(
        port,
        success_stream(Arc::new(AtomicUsize::new(0))),
        session,
    );
    let original_model = harness.agent().state().model.clone().unwrap();

    assert!(harness.set_model(faux_model()).await.is_err());
    assert_eq!(harness.agent().state().model.as_ref().unwrap().id, original_model.id);
    assert!(harness.move_to(None, None).await.is_err());
    assert_eq!(harness.session().leaf_id().await.unwrap().as_deref(), Some(root.as_str()));
}

#[tokio::test]
async fn shutdown_waits_for_cancelled_run_settlement_before_session_shutdown() {
    let port = Arc::new(RecordingPort::default());
    let release = Arc::new(tokio::sync::Notify::new());
    let stream_fn: StreamFn = {
        let release = release.clone();
        Arc::new(move |_, _, _| {
            let (stream, sender) = AssistantMessageEventStream::new();
            let release = release.clone();
            tokio::spawn(async move {
                release.notified().await;
                drop(sender);
            });
            stream
        })
    };
    let harness = Arc::new(harness_with_port(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
    ));
    let prompt_harness = harness.clone();
    let prompt = tokio::spawn(async move { prompt_harness.prompt("hello").await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !harness.agent().is_streaming() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    harness.shutdown_runtime_extensions().await;
    release.notify_waiters();
    let _ = prompt.await.unwrap();

    let events = port.events();
    let settled = events
        .iter()
        .position(|event| *event == ExtensionLifecycleEvent::RunSettled)
        .unwrap();
    let shutdown = events
        .iter()
        .position(|event| *event == ExtensionLifecycleEvent::SessionShutdown)
        .unwrap();
    assert!(settled < shutdown);
}

#[tokio::test]
async fn before_run_patch_persist_failure_returns_agent_error() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::BeforeRun,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::PatchRunContext,
                payload: serde_json::json!({
                    "messages": [user_message("injected context")],
                }),
            }],
        },
    );
    let harness = harness_with_port(
        port,
        success_stream(Arc::new(AtomicUsize::new(0))),
        Session::new(Arc::new(FailingAppendStorage::new())),
    );

    let error = harness.prompt("hello").await.unwrap_err();

    assert!(error.to_string().contains("persist before-run messages"));
}

struct ReentrantGuardedPort {
    runtime: OnceLock<Arc<HarnessRuntimeExtensions>>,
}

#[async_trait]
impl RuntimeRequestExtensionPort for ReentrantGuardedPort {
    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            self.runtime
                .get()
                .unwrap()
                .observe_session_operation(
                    ExtensionLifecycleEvent::SessionStart,
                    serde_json::json!({}),
                )
                .await;
        }
        Ok(empty_batch())
    }
}

macro_rules! impl_reentrant_guarded_noop {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for ReentrantGuardedPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                Ok(empty_batch())
            }
        }
    };
}

impl_reentrant_guarded_noop!(RuntimeSessionExtensionPort, invoke_session);
impl_reentrant_guarded_noop!(RuntimeRunExtensionPort, invoke_run);
impl_reentrant_guarded_noop!(RuntimeMessageExtensionPort, invoke_message);
impl_reentrant_guarded_noop!(RuntimeToolExtensionPort, invoke_tool);
impl_reentrant_guarded_noop!(RuntimeCompactionExtensionPort, invoke_compaction);

#[tokio::test]
async fn runtime_ext_guarded_rejects_reentrant_dispatch() {
    let port = Arc::new(ReentrantGuardedPort {
        runtime: OnceLock::new(),
    });
    let runtime = Arc::new(coverage_runtime_with_port(port.clone()));
    port.runtime
        .set(Arc::clone(&runtime))
        .unwrap_or_else(|_| unreachable!());

    let original = user_message("outer");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}
