use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use theway_contract::extension::{
    ExtensionAction, ExtensionActionBatch, ExtensionActionKind, ExtensionErrorCode,
    ExtensionErrorEnvelope, ExtensionGateDecision, ExtensionHookClass, ExtensionLifecycleEvent,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ErrorReason, ProviderRequestFailure, ProviderRequestFailureStage,
    ProviderRequestHeaders, ProviderRequestPayload, ProviderResponseMetadata, ProviderWireFormat,
    StopReason, ToolCall, Usage,
};

use super::*;
use crate::agent::assembly::runtime_extensions::HarnessRuntimeExtensions;
use crate::agent::model_request::{NormalizedGenerationOptions, NormalizedModelRequestDraft};
use crate::types::{AfterToolCallContext, AgentContext, AgentToolResult};
use crate::agent::runtime_extensions::{
    ExtensionModelContextProjection, RawRuntimeExtensionResult, RuntimeCompactionExtensionPort,
    RuntimeExtensionInvocation, RuntimeExtensionPort, RuntimeExtensionResult,
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort, RuntimeRunExtensionPort,
    RuntimeSessionExtensionPort, RuntimeToolExtensionPort, ValidatedObserveResult,
    ValidatedRuntimeExtensionResult,
};

mod compaction_lifecycle;
mod message_seams;
mod message_tool;
mod normalized_request;
mod provider_interceptor;
mod provider_seams;
mod reentrancy;
mod request_seams;
mod run_lifecycle;
mod session_run_seams;
mod tool_seams;
mod transform_context;
mod transform_input;

#[derive(Clone, Debug)]
struct Record {
    event: ExtensionLifecycleEvent,
    sequence: u64,
}

#[derive(Default)]
struct RecordingPort {
    records: Mutex<Vec<Record>>,
    responses:
        Mutex<BTreeMap<(ExtensionLifecycleEvent, ExtensionHookClass), ExtensionActionBatch>>,
    chain_follow_ups: AtomicBool,
    next_follow_up: AtomicUsize,
}

impl RecordingPort {
    fn respond(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
        response: ExtensionActionBatch,
    ) {
        self.responses.lock().insert((event, class), response);
    }

    fn events(&self) -> Vec<ExtensionLifecycleEvent> {
        self.records
            .lock()
            .iter()
            .map(|record| record.event)
            .collect()
    }

    fn enable_unbounded_follow_up_chain(&self) {
        self.chain_follow_ups.store(true, Ordering::Release);
    }

    fn invoke(&self, invocation: RuntimeExtensionInvocation) -> RawRuntimeExtensionResult {
        self.records.lock().push(Record {
            event: invocation.event(),
            sequence: invocation.context().sequence,
        });
        if self.chain_follow_ups.load(Ordering::Acquire)
            && invocation.event() == ExtensionLifecycleEvent::Context
        {
            let ordinal = self.next_follow_up.fetch_add(1, Ordering::Relaxed);
            return Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": format!("chain-{ordinal}"),
                        "message": user_message(&format!("follow up {ordinal}")),
                    }),
                }],
            });
        }
        Ok(self
            .responses
            .lock()
            .get(&(invocation.event(), invocation.class()))
            .cloned()
            .unwrap_or_else(empty_batch))
    }
}

fn empty_batch() -> ExtensionActionBatch {
    ExtensionActionBatch {
        decision: None,
        actions: Vec::new(),
    }
}

macro_rules! impl_recording_port {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for RecordingPort {
            async fn $method(
                &self,
                invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                self.invoke(invocation)
            }
        }
    };
}

impl_recording_port!(RuntimeSessionExtensionPort, invoke_session);
impl_recording_port!(RuntimeRunExtensionPort, invoke_run);
impl_recording_port!(RuntimeRequestExtensionPort, invoke_request);
impl_recording_port!(RuntimeMessageExtensionPort, invoke_message);
impl_recording_port!(RuntimeToolExtensionPort, invoke_tool);
impl_recording_port!(RuntimeCompactionExtensionPort, invoke_compaction);

fn assistant(text: &str) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn success_stream(calls: Arc<AtomicUsize>) -> StreamFn {
    Arc::new(move |_, _, _| {
        calls.fetch_add(1, Ordering::Relaxed);
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let message = assistant("ok");
        sender.push(AssistantMessageEvent::Start {
            partial: message.clone(),
        });
        sender.push(AssistantMessageEvent::Done {
            reason: DoneReason::Stop,
            message,
        });
        stream
    })
}

fn harness_with_port(
    port: Arc<RecordingPort>,
    stream_fn: StreamFn,
    session: Session,
) -> AgentHarness {
    let mut options = AgentHarnessOptions::new(faux_model(), session);
    options.observation_context.session_id = Some("session-1".into());
    options.runtime_extension_cwd = "/workspace".into();
    options.runtime_extensions = port;
    options.stream_fn = Some(stream_fn);
    AgentHarness::new(options)
}

struct FailingAppendStorage {
    inner: MemorySessionStorage,
}

impl FailingAppendStorage {
    fn new() -> Self {
        Self {
            inner: MemorySessionStorage::new(),
        }
    }
}

#[async_trait]
impl SessionStorage for FailingAppendStorage {
    async fn get_metadata_json(&self) -> Result<serde_json::Value, SessionError> {
        self.inner.get_metadata_json().await
    }

    async fn get_leaf_id(&self) -> Result<Option<String>, SessionError> {
        self.inner.get_leaf_id().await
    }

    async fn set_leaf_id(&self, id: Option<String>) -> Result<(), SessionError> {
        self.inner.set_leaf_id(id).await
    }

    async fn create_entry_id(&self) -> Result<String, SessionError> {
        self.inner.create_entry_id().await
    }

    async fn append_entry(&self, _entry: SessionTreeEntry) -> Result<(), SessionError> {
        Err(SessionError {
            code: SessionErrorCode::StorageFailure,
            message: "append failed".into(),
        })
    }

    async fn get_entry(&self, id: &str) -> Result<Option<SessionTreeEntry>, SessionError> {
        self.inner.get_entry(id).await
    }

    async fn get_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_entries().await
    }

    async fn get_path_to_root(
        &self,
        leaf_id: Option<&str>,
    ) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.get_path_to_root(leaf_id).await
    }

    async fn find_entries(&self, entry_type: &str) -> Result<Vec<SessionTreeEntry>, SessionError> {
        self.inner.find_entries(entry_type).await
    }

    async fn get_label(&self, id: &str) -> Result<Option<String>, SessionError> {
        self.inner.get_label(id).await
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Branch coverage for `agent::assembly::runtime_extensions*`
// ──────────────────────────────────────────────────────────────────────────────────────────

type CoverageHandler = Arc<dyn Fn(&RuntimeExtensionInvocation) -> RawRuntimeExtensionResult + Send + Sync>;

struct CoverageFnPort {
    handler: CoverageHandler,
    request_has_hook: bool,
}

impl CoverageFnPort {
    fn new(
        handler: impl Fn(&RuntimeExtensionInvocation) -> RawRuntimeExtensionResult + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Arc::new(handler),
            request_has_hook: true,
        }
    }

    fn with_request_hook(mut self, has: bool) -> Self {
        self.request_has_hook = has;
        self
    }

    fn invoke(&self, invocation: RuntimeExtensionInvocation) -> RawRuntimeExtensionResult {
        (self.handler)(&invocation)
    }
}

macro_rules! impl_coverage_fn_port_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for CoverageFnPort {
            async fn $method(
                &self,
                invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                self.invoke(invocation)
            }
        }
    };
}

impl_coverage_fn_port_domain!(RuntimeSessionExtensionPort, invoke_session);
impl_coverage_fn_port_domain!(RuntimeRunExtensionPort, invoke_run);
impl_coverage_fn_port_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_coverage_fn_port_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_coverage_fn_port_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait]
impl RuntimeRequestExtensionPort for CoverageFnPort {
    fn has_request_hook(
        &self,
        _event: ExtensionLifecycleEvent,
        _class: ExtensionHookClass,
    ) -> bool {
        self.request_has_hook
    }

    async fn invoke_request(
        &self,
        invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        self.invoke(invocation)
    }
}

fn coverage_runtime_with_handler(
    cwd: &str,
    handler: impl Fn(&RuntimeExtensionInvocation) -> RawRuntimeExtensionResult + Send + Sync + 'static,
) -> HarnessRuntimeExtensions {
    HarnessRuntimeExtensions::new(
        Arc::new(CoverageFnPort::new(handler)),
        "coverage-session".into(),
        cwd.into(),
        false,
        None,
        ExtensionModelContextProjection::default(),
    )
}

fn coverage_runtime_with_cwd(cwd: &str) -> HarnessRuntimeExtensions {
    coverage_runtime_with_handler(cwd, |_| Ok(empty_batch()))
}

fn coverage_runtime_with_port(port: Arc<dyn RuntimeExtensionPort>) -> HarnessRuntimeExtensions {
    HarnessRuntimeExtensions::new(
        port,
        "coverage-session".into(),
        "/workspace".into(),
        false,
        None,
        ExtensionModelContextProjection::default(),
    )
}

fn coverage_agent_message_eq(left: &AgentMessage, right: &AgentMessage) -> bool {
    serde_json::to_value(left).unwrap() == serde_json::to_value(right).unwrap()
}

fn coverage_assert_messages_eq(left: Vec<AgentMessage>, right: Vec<AgentMessage>) {
    assert_eq!(
        serde_json::to_value(left).unwrap(),
        serde_json::to_value(right).unwrap()
    );
}

fn coverage_assistant_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::Assistant(assistant(text)))
}

fn coverage_follow_up_action(id: &str, message: AgentMessage) -> ExtensionAction {
    ExtensionAction {
        kind: ExtensionActionKind::EnqueueFollowUp,
        payload: serde_json::json!({
            "followUpId": id,
            "message": message,
        }),
    }
}

fn coverage_error(code: ExtensionErrorCode, message: &str) -> ExtensionErrorEnvelope {
    ExtensionErrorEnvelope::new(code, message)
}
