use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use theway_llm_provider::Tool;

use super::*;
use crate::agent::assembly::runtime_extensions::HarnessRuntimeExtensions;
use crate::agent::model_request::{
    NormalizedGenerationOptions, NormalizedModelRequestDraft,
};
use crate::agent::runtime_extensions::ExtensionModelContextProjection;

#[derive(Clone, Copy)]
enum RequestBehavior {
    Filter(&'static str),
    Pass,
    Invalid,
    NoSubscribers,
}

struct RequestPort {
    behavior: RequestBehavior,
    invocations: AtomicUsize,
}

impl RequestPort {
    fn new(behavior: RequestBehavior) -> Self {
        Self {
            behavior,
            invocations: AtomicUsize::new(0),
        }
    }
}

fn request_batch(actions: Vec<ExtensionAction>) -> RawRuntimeExtensionResult {
    Ok(ExtensionActionBatch {
        decision: None,
        actions,
    })
}

macro_rules! impl_empty_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for RequestPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                request_batch(Vec::new())
            }
        }
    };
}

impl_empty_domain!(RuntimeSessionExtensionPort, invoke_session);
impl_empty_domain!(RuntimeRunExtensionPort, invoke_run);
impl_empty_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_empty_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_empty_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait]
impl RuntimeRequestExtensionPort for RequestPort {
    fn has_request_hook(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
    ) -> bool {
        event == ExtensionLifecycleEvent::BeforeModelRequest
            && class == ExtensionHookClass::Transform
            && !matches!(self.behavior, RequestBehavior::NoSubscribers)
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        let mut request: NormalizedModelRequestDraft =
            serde_json::from_value(invocation.payload()["request"].clone()).unwrap();
        match self.behavior {
            RequestBehavior::Filter(name) => {
                request.visible_tools.retain(|tool| tool.name == name);
                request
                    .executable_tool_names
                    .retain(|tool_name| tool_name == name);
                request.system_instructions = Some(format!("session policy: {name}"));
                request_batch(vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceModelRequest,
                    payload: serde_json::json!({"request": request}),
                }])
            }
            RequestBehavior::Pass => request_batch(Vec::new()),
            RequestBehavior::Invalid => {
                request.provider = "different-provider".into();
                request.visible_tools = vec![tool("unregistered")];
                request.executable_tool_names = vec!["unregistered".into()];
                request_batch(vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceModelRequest,
                    payload: serde_json::json!({"request": request}),
                }])
            }
            RequestBehavior::NoSubscribers => {
                panic!("request script must not run without subscribers")
            }
        }
    }
}

fn tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("{name} tool"),
        parameters: serde_json::json!({"type": "object"}),
    }
}

fn draft() -> NormalizedModelRequestDraft {
    NormalizedModelRequestDraft {
        provider: "test-provider".into(),
        model: "test-model".into(),
        system_instructions: Some("base system".into()),
        messages: Vec::new(),
        visible_tools: vec![tool("bash"), tool("edit")],
        executable_tool_names: vec!["bash".into(), "edit".into()],
        generation_options: NormalizedGenerationOptions::default(),
    }
}

fn runtime(session_id: &str, port: Arc<RequestPort>) -> HarnessRuntimeExtensions {
    HarnessRuntimeExtensions::new(
        port,
        session_id.into(),
        "/workspace".into(),
        false,
        Some(theway_contract::extension::ExtensionModelRef {
            provider: "test-provider".into(),
            model: "test-model".into(),
        }),
        ExtensionModelContextProjection::default(),
    )
}

#[tokio::test]
async fn concurrent_sessions_derive_independent_request_local_catalogs() {
    let filtering_port = Arc::new(RequestPort::new(RequestBehavior::Filter("bash")));
    let pass_port = Arc::new(RequestPort::new(RequestBehavior::Pass));
    let filtering_runtime = runtime("session-filtering", Arc::clone(&filtering_port));
    let pass_runtime = runtime("session-pass", Arc::clone(&pass_port));
    let base = draft();

    let (filtered, unchanged) = tokio::join!(
        filtering_runtime.before_model_request(
            base.clone(),
            16_384,
            tokio_util::sync::CancellationToken::new(),
        ),
        pass_runtime.before_model_request(
            base.clone(),
            16_384,
            tokio_util::sync::CancellationToken::new(),
        )
    );

    assert_eq!(filtered.visible_tools.len(), 1);
    assert_eq!(filtered.visible_tools[0].name, "bash");
    assert_eq!(filtered.executable_tool_names, ["bash"]);
    assert_eq!(unchanged.visible_tools.len(), 2);
    assert_eq!(unchanged.executable_tool_names, ["bash", "edit"]);
    assert_eq!(base.visible_tools.len(), 2);
    assert_eq!(base.system_instructions.as_deref(), Some("base system"));
    assert_eq!(filtering_port.invocations.load(Ordering::Relaxed), 1);
    assert_eq!(pass_port.invocations.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn invalid_request_action_is_rejected_atomically() {
    let port = Arc::new(RequestPort::new(RequestBehavior::Invalid));
    let runtime = runtime("session-invalid", port);
    let base = draft();

    let accepted = runtime
        .before_model_request(
            base.clone(),
            16_384,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

    assert_eq!(
        serde_json::to_value(accepted).unwrap(),
        serde_json::to_value(base).unwrap()
    );
}

#[tokio::test]
async fn no_subscriber_fast_path_skips_request_script_dispatch() {
    let port = Arc::new(RequestPort::new(RequestBehavior::NoSubscribers));
    let runtime = runtime("session-no-subscriber", Arc::clone(&port));
    let base = draft();

    let accepted = runtime
        .before_model_request(
            base.clone(),
            16_384,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

    assert_eq!(port.invocations.load(Ordering::Relaxed), 0);
    assert_eq!(
        serde_json::to_value(accepted).unwrap(),
        serde_json::to_value(base).unwrap()
    );
}
