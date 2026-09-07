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
mod message_tool;
mod normalized_request;
mod provider_interceptor;

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

#[tokio::test]
async fn handled_extension_input_returns_outcome_without_dispatching_provider() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::EmitCommandOutcome,
                payload: serde_json::json!({
                    "status": "success",
                    "message": "handled",
                }),
            }],
        },
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = harness_with_port(
        port.clone(),
        success_stream(calls.clone()),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );
    let mut events = harness.subscribe_session_broadcast();

    harness.prompt("/extension-command").await.unwrap();

    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(harness.agent().state().messages.is_empty());
    assert!(port.events().contains(&ExtensionLifecycleEvent::Input));
    assert!(!port.events().contains(&ExtensionLifecycleEvent::BeforeRun));
    assert!(std::iter::from_fn(|| events.try_recv().ok())
        .any(|event| matches!(event, SessionEvent::ExtensionCommandOutcome { .. })));
}

#[tokio::test]
async fn input_replacement_is_the_message_used_by_the_run_and_transcript() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::ReplaceInput,
                payload: serde_json::json!({"message": user_message("rewritten")}),
            }],
        },
    );
    let harness = harness_with_port(
        port,
        success_stream(Arc::new(AtomicUsize::new(0))),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    harness.prompt("original").await.unwrap();

    assert_eq!(extract_user_prompt_text(&harness.agent().state().messages[0]), Some("rewritten".into()));
}

#[tokio::test]
async fn successful_run_lifecycle_is_ordered_and_sequences_are_monotonic() {
    let port = Arc::new(RecordingPort::default());
    let harness = harness_with_port(
        port.clone(),
        success_stream(Arc::new(AtomicUsize::new(0))),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    harness.prompt("hello").await.unwrap();

    assert_eq!(
        port.events(),
        vec![
            ExtensionLifecycleEvent::SessionStart,
            ExtensionLifecycleEvent::Input,
            ExtensionLifecycleEvent::BeforeRun,
            ExtensionLifecycleEvent::RunStarted,
            ExtensionLifecycleEvent::MessageStart,
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionLifecycleEvent::TurnStarted,
            ExtensionLifecycleEvent::Context,
            ExtensionLifecycleEvent::BeforeModelRequest,
            ExtensionLifecycleEvent::MessageStart,
            ExtensionLifecycleEvent::MessageEnd,
            ExtensionLifecycleEvent::TurnCompleted,
            ExtensionLifecycleEvent::RunEnded,
            ExtensionLifecycleEvent::RunSettled,
        ]
    );
    let sequences = port
        .records
        .lock()
        .iter()
        .map(|record| record.sequence)
        .collect::<Vec<_>>();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn before_run_patch_persists_messages_and_limits_system_prompt_to_the_run() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::BeforeRun,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::PatchRunContext,
                payload: serde_json::json!({
                    "systemPrompt": "temporary instructions",
                    "messages": [user_message("injected context")],
                }),
            }],
        },
    );
    let request = Arc::new(Mutex::new(None));
    let stream_fn: StreamFn = {
        let request = Arc::clone(&request);
        Arc::new(move |_, context, _| {
            *request.lock() = Some((context.system_prompt.clone(), context.messages.len()));
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let message = assistant("ok");
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
            stream
        })
    };
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    let harness = harness_with_port(port, stream_fn, session.clone());

    harness.prompt("hello").await.unwrap();

    assert_eq!(
        *request.lock(),
        Some((Some("temporary instructions".into()), 2))
    );
    assert_eq!(
        extract_user_prompt_text(&harness.agent().state().messages[0]),
        Some("injected context".into())
    );
    assert_eq!(
        extract_user_prompt_text(&session.build_context().await.unwrap().messages[0]),
        Some("injected context".into())
    );
    assert_eq!(harness.agent().state().system_prompt, "");
}

#[tokio::test]
async fn provider_failure_emits_run_error_between_ended_and_settled() {
    let port = Arc::new(RecordingPort::default());
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut message = assistant("");
        message.stop_reason = StopReason::Error;
        message.error_message = Some("provider failed".into());
        sender.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: message,
        });
        stream
    });
    let harness = harness_with_port(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    assert!(harness.prompt("hello").await.is_err());

    let terminal = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::RunEnded
                    | ExtensionLifecycleEvent::RunError
                    | ExtensionLifecycleEvent::RunSettled
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminal,
        vec![
            ExtensionLifecycleEvent::RunEnded,
            ExtensionLifecycleEvent::RunError,
            ExtensionLifecycleEvent::RunSettled,
        ]
    );
}

#[tokio::test]
async fn persistence_failure_uses_the_same_error_then_settled_terminal_order() {
    let port = Arc::new(RecordingPort::default());
    let harness = harness_with_port(
        port.clone(),
        success_stream(Arc::new(AtomicUsize::new(0))),
        Session::new(Arc::new(FailingAppendStorage::new())),
    );

    let error = harness.prompt("hello").await.unwrap_err();

    assert!(error.to_string().contains("session append message"));
    let terminal = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::RunEnded
                    | ExtensionLifecycleEvent::RunError
                    | ExtensionLifecycleEvent::RunSettled
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        terminal,
        vec![
            ExtensionLifecycleEvent::RunEnded,
            ExtensionLifecycleEvent::RunError,
            ExtensionLifecycleEvent::RunSettled,
        ]
    );
}

#[tokio::test]
async fn context_transform_is_request_local_and_runs_between_turn_boundaries() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::Context,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::ReplaceContext,
                payload: serde_json::json!({"messages": []}),
            }],
        },
    );
    let visible_messages = Arc::new(AtomicUsize::new(usize::MAX));
    let stream_fn: StreamFn = {
        let visible_messages = visible_messages.clone();
        Arc::new(move |_, context, _| {
            visible_messages.store(context.messages.len(), Ordering::Relaxed);
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let message = assistant("ok");
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message,
            });
            stream
        })
    };
    let harness = harness_with_port(
        port,
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    harness.prompt("private for this request").await.unwrap();

    assert_eq!(visible_messages.load(Ordering::Relaxed), 0);
    assert_eq!(harness.agent().state().messages.len(), 2);
}

#[tokio::test]
async fn tool_use_run_emits_complete_turn_context_order_before_the_next_turn() {
    let port = Arc::new(RecordingPort::default());
    let call = Arc::new(AtomicUsize::new(0));
    let stream_fn: StreamFn = {
        let call = call.clone();
        Arc::new(move |_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let index = call.fetch_add(1, Ordering::Relaxed);
            if index == 0 {
                let mut message = assistant("");
                message.content = vec![ContentBlock::ToolCall(theway_llm_provider::ToolCall {
                    id: "call-1".into(),
                    name: "missing-tool".into(),
                    arguments: serde_json::Map::new(),
                    thought_signature: None,
                })];
                message.stop_reason = StopReason::ToolUse;
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::ToolUse,
                    message,
                });
            } else {
                let message = assistant("finished");
                sender.push(AssistantMessageEvent::Done {
                    reason: DoneReason::Stop,
                    message,
                });
            }
            stream
        })
    };
    let harness = harness_with_port(
        port.clone(),
        stream_fn,
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    harness.prompt("use a tool").await.unwrap();

    let turns = port
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                ExtensionLifecycleEvent::TurnStarted
                    | ExtensionLifecycleEvent::Context
                    | ExtensionLifecycleEvent::TurnCompleted
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        turns,
        vec![
            ExtensionLifecycleEvent::TurnStarted,
            ExtensionLifecycleEvent::Context,
            ExtensionLifecycleEvent::TurnCompleted,
            ExtensionLifecycleEvent::TurnStarted,
            ExtensionLifecycleEvent::Context,
            ExtensionLifecycleEvent::TurnCompleted,
        ]
    );
}

#[tokio::test]
async fn extension_follow_up_is_deduplicated_and_starts_only_after_run_settled() {
    let port = Arc::new(RecordingPort::default());
    port.respond(
        ExtensionLifecycleEvent::Context,
        ExtensionHookClass::Transform,
        ExtensionActionBatch {
            decision: None,
            actions: vec![ExtensionAction {
                kind: ExtensionActionKind::EnqueueFollowUp,
                payload: serde_json::json!({
                    "followUpId": "once",
                    "message": user_message("follow up"),
                }),
            }],
        },
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = harness_with_port(
        port.clone(),
        success_stream(calls.clone()),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    harness.prompt("initial").await.unwrap();

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    let events = port.events();
    let first_settled = events
        .iter()
        .position(|event| *event == ExtensionLifecycleEvent::RunSettled)
        .unwrap();
    let second_before_run = events
        .iter()
        .enumerate()
        .find(|(index, event)| {
            *index > first_settled && **event == ExtensionLifecycleEvent::BeforeRun
        })
        .map(|(index, _)| index)
        .unwrap();
    assert!(first_settled < second_before_run);
}

#[tokio::test]
async fn unbounded_extension_follow_up_chain_is_stopped_after_the_declared_cap() {
    let port = Arc::new(RecordingPort::default());
    port.enable_unbounded_follow_up_chain();
    let calls = Arc::new(AtomicUsize::new(0));
    let harness = harness_with_port(
        port,
        success_stream(calls.clone()),
        Session::new(Arc::new(MemorySessionStorage::new())),
    );

    let error = harness.prompt("initial").await.unwrap_err();

    assert!(error.to_string().contains("follow-up chain exceeded 16"));
    assert_eq!(calls.load(Ordering::Relaxed), 17);
}

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

// ── lifecycle idempotency + invocation error paths ─────────────────────────────────────

#[tokio::test]
async fn runtime_ext_ensure_session_start_second_call_is_noop() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.ensure_session_start().await;
    runtime.ensure_session_start().await;
}

#[tokio::test]
async fn runtime_ext_ensure_session_start_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_runtime_with_cwd("");
    runtime.ensure_session_start().await;
}

#[tokio::test]
async fn runtime_ext_shutdown_second_call_is_noop() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.shutdown().await;
    runtime.shutdown().await;
}

#[tokio::test]
async fn runtime_ext_shutdown_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_runtime_with_cwd("");
    runtime.shutdown().await;
}

// ── transform_input fallbacks ──────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_transform_input_invocation_error_returns_original() {
    let runtime = coverage_runtime_with_cwd("");
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_dispatch_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "input failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_payload_missing_message_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_invalid_json_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({"message": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_replace_role_mismatch_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceInput,
                    payload: serde_json::json!({"message": coverage_assistant_message("assistant")}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_emit_command_outcome_invalid_payload_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EmitCommandOutcome,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_valid_is_accepted() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![coverage_follow_up_action("input-follow-up", user_message("follow"))],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_invalid_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_enqueue_follow_up_capacity_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("input-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

// parse_follow_up branches exercised through transform_input
#[tokio::test]
async fn runtime_ext_transform_input_rejects_empty_follow_up_id() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "",
                        "message": user_message("follow"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_rejects_oversized_follow_up_id() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "x".repeat(129),
                        "message": user_message("follow"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

#[tokio::test]
async fn runtime_ext_transform_input_rejects_non_user_follow_up_message() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Input {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({
                        "followUpId": "valid-id",
                        "message": coverage_assistant_message("assistant"),
                    }),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let original = user_message("original");
    let outcome = runtime.transform_input(original.clone()).await;
    assert!(matches!(
        outcome,
        crate::agent::assembly::runtime_extensions::InputTransformOutcome::Run(message)
            if coverage_agent_message_eq(&message, &original)
    ));
}

// ── transform_context fallbacks ────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_transform_context_invocation_error_returns_messages() {
    let runtime = coverage_runtime_with_cwd("");
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_dispatch_error_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "context failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_invalid_follow_up_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

#[tokio::test]
async fn runtime_ext_transform_context_enqueue_follow_up_capacity_error_returns_messages() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::Context {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("context-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let messages = vec![user_message("a")];
    coverage_assert_messages_eq(
        runtime
            .transform_context(messages.clone(), CancellationToken::new())
            .await,
        messages,
    );
}

// Dedup-eviction loop: after 8 full batches (256 unique ids) the 257th unique id
// triggers `while seen_order.len() > FOLLOW_UP_DEDUP_CAPACITY` and `pop_front()`.
#[tokio::test]
async fn runtime_ext_follow_up_dedup_evicts_oldest_seen_id_after_capacity() {
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = coverage_runtime_with_handler("/workspace", {
        let calls = Arc::clone(&calls);
        move |invocation| {
            if invocation.event() == ExtensionLifecycleEvent::Context {
                let batch_index = calls.fetch_add(1, Ordering::Relaxed);
                let actions = (0..32)
                    .map(|index| {
                        coverage_follow_up_action(
                            &format!("dedup-{}", batch_index * 32 + index),
                            user_message("follow"),
                        )
                    })
                    .collect();
                Ok(ExtensionActionBatch {
                    decision: None,
                    actions,
                })
            } else {
                Ok(empty_batch())
            }
        }
    });

    for _ in 0..9 {
        runtime
            .transform_context(vec![user_message("a")], CancellationToken::new())
            .await;
        while runtime.take_follow_up().is_some() {}
    }
}

// ── remaining invocation-error paths ───────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_before_model_selection_invocation_error_is_reported() {
    let runtime = coverage_runtime_with_cwd("");
    assert!(runtime.before_model_selection(&faux_model()).await.is_err());
}

#[tokio::test]
async fn runtime_ext_model_selected_handles_success_and_invocation_error() {
    let runtime = coverage_runtime_with_cwd("/workspace");
    runtime.model_selected(&faux_model()).await;

    let runtime = coverage_runtime_with_cwd("");
    runtime.model_selected(&faux_model()).await;
}

#[tokio::test]
async fn runtime_ext_observe_session_operation_ignores_invocation_error() {
    let runtime = coverage_runtime_with_cwd("");
    runtime
        .observe_session_operation(ExtensionLifecycleEvent::SessionStart, serde_json::json!({}))
        .await;
}

#[tokio::test]
async fn runtime_ext_observe_run_ignores_invocation_error() {
    let runtime = coverage_runtime_with_cwd("");
    runtime
        .observe_run(ExtensionLifecycleEvent::RunStarted, serde_json::json!({}), false)
        .await;
}

#[tokio::test]
async fn runtime_ext_before_run_invocation_error_returns_default_patch() {
    let runtime = coverage_runtime_with_cwd("");
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_dispatch_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "before-run failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_patch_deserialize_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::PatchRunContext,
                    payload: serde_json::json!({"systemPrompt": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_invalid_follow_up_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_before_run_enqueue_follow_up_capacity_error_returns_default_patch() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeRun {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("before-run-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let patch = runtime.before_run().await;
    assert!(patch.messages.is_empty());
    assert!(patch.system_prompt.is_none());
}

#[tokio::test]
async fn runtime_ext_gate_allows_enqueues_follow_up_and_allows() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeSessionSwitch
            && invocation.class() == ExtensionHookClass::Gate
        {
            Ok(ExtensionActionBatch {
                decision: Some(ExtensionGateDecision::Allow),
                actions: vec![coverage_follow_up_action("gate-follow-up", user_message("follow"))],
            })
        } else {
            Ok(empty_batch())
        }
    });
    runtime
        .gate_session_operation(ExtensionLifecycleEvent::BeforeSessionSwitch, serde_json::json!({}))
        .await
        .unwrap();
    assert!(runtime.take_follow_up().is_some());
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

// ── message seams ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn runtime_ext_observe_message_update_without_message_id_is_noop() {
    let runtime = Arc::new(coverage_runtime_with_cwd("/workspace"));
    let (listener, _) = runtime.make_loop_listener();
    listener(
        crate::types::LoopEvent::MessageUpdate {
            message: user_message("hello"),
            assistant_message_event: AssistantMessageEvent::Start {
                partial: assistant(""),
            },
        },
        CancellationToken::new(),
    )
    .await;
}

#[tokio::test]
async fn runtime_ext_transform_message_invocation_error_returns_message() {
    let runtime = coverage_runtime_with_cwd("");
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_dispatch_error_returns_message() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "message failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_replace_payload_missing_message_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_replace_invalid_json_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({"message": 42}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_role_mismatch_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceMessage,
                    payload: serde_json::json!({"message": coverage_assistant_message("assistant")}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_invalid_follow_up_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

#[tokio::test]
async fn runtime_ext_transform_message_enqueue_follow_up_capacity_error_returns_original() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::MessageEnd
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("message-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let message = user_message("hello");
    let transformed = runtime
        .transform_message(message.clone(), CancellationToken::new())
        .await;
    assert!(coverage_agent_message_eq(&transformed, &message));
}

// ── tool seams ─────────────────────────────────────────────────────────────────────────

fn coverage_tool_call() -> ToolCall {
    ToolCall {
        id: "tool-1".into(),
        name: "coverage-tool".into(),
        arguments: serde_json::Map::new(),
        thought_signature: None,
    }
}

fn coverage_after_tool_context() -> AfterToolCallContext {
    AfterToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        result: AgentToolResult::default(),
        is_error: false,
        context: AgentContext::default(),
    }
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_invocation_error_returns_default() {
    let runtime = coverage_runtime_with_cwd("");
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_dispatch_error_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "tool result failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_invalid_follow_up_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_transform_tool_result_enqueue_follow_up_capacity_error_returns_default() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolResult
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("tool-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let result = runtime
        .transform_tool_result(&coverage_after_tool_context(), &CancellationToken::new())
        .await;
    assert!(result.content.is_none());
    assert!(result.is_error.is_none());
    assert!(result.terminate.is_none());
}

#[tokio::test]
async fn runtime_ext_observe_tool_execution_ignores_invocation_error_with_empty_cwd() {
    let runtime = Arc::new(coverage_runtime_with_cwd(""));
    let (listener, _) = runtime.make_loop_listener();
    listener(
        crate::types::LoopEvent::ToolExecutionStart {
            tool_call_id: "tool-1".into(),
            tool_name: "coverage-tool".into(),
            args: serde_json::json!({}),
        },
        CancellationToken::new(),
    )
    .await;
}

#[tokio::test]
async fn runtime_ext_before_tool_call_blocked_error_non_cancelled() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolCall
            && invocation.class() == ExtensionHookClass::Gate
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "limit reached"))
        } else {
            Ok(empty_batch())
        }
    });
    let context = crate::types::BeforeToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        context: AgentContext::default(),
    };
    let result = runtime.before_tool_call(&context, &CancellationToken::new()).await;
    assert!(result.block);
    assert!(result.reason.unwrap().contains("resource_limit"));
}

#[tokio::test]
async fn runtime_ext_before_tool_call_blocked_error_malformed_cancelled() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::ToolCall
            && invocation.class() == ExtensionHookClass::Gate
        {
            Err(coverage_error(ExtensionErrorCode::Cancelled, "no separator here"))
        } else {
            Ok(empty_batch())
        }
    });
    let context = crate::types::BeforeToolCallContext {
        assistant_message: assistant("assistant"),
        tool_call: coverage_tool_call(),
        args: serde_json::json!({}),
        context: AgentContext::default(),
    };
    let result = runtime.before_tool_call(&context, &CancellationToken::new()).await;
    assert!(result.block);
    assert!(result.reason.unwrap().contains("cancelled"));
}

// ── request seams ──────────────────────────────────────────────────────────────────────

fn coverage_request_draft() -> NormalizedModelRequestDraft {
    NormalizedModelRequestDraft {
        provider: "test-provider".into(),
        model: "test-model".into(),
        system_instructions: Some("base system".into()),
        messages: Vec::new(),
        visible_tools: vec![
            theway_llm_provider::Tool {
                name: "bash".into(),
                description: "bash tool".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
            theway_llm_provider::Tool {
                name: "edit".into(),
                description: "edit tool".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        ],
        executable_tool_names: vec!["bash".into(), "edit".into()],
        generation_options: NormalizedGenerationOptions::default(),
    }
}

fn coverage_assert_request_unchanged(
    accepted: NormalizedModelRequestDraft,
    base: &NormalizedModelRequestDraft,
) {
    assert_eq!(
        serde_json::to_value(accepted).unwrap(),
        serde_json::to_value(base).unwrap()
    );
}

#[tokio::test]
async fn runtime_ext_before_model_request_invocation_error_returns_request() {
    let runtime = coverage_runtime_with_cwd("");
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_dispatch_error_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            Err(coverage_error(ExtensionErrorCode::ResourceLimit, "request failed"))
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_replace_payload_invalid_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::ReplaceModelRequest,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_invalid_follow_up_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            Ok(ExtensionActionBatch {
                decision: None,
                actions: vec![ExtensionAction {
                    kind: ExtensionActionKind::EnqueueFollowUp,
                    payload: serde_json::json!({}),
                }],
            })
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

#[tokio::test]
async fn runtime_ext_before_model_request_enqueue_follow_up_capacity_error_returns_request() {
    let runtime = coverage_runtime_with_handler("/workspace", |invocation| {
        if invocation.event() == ExtensionLifecycleEvent::BeforeModelRequest
            && invocation.class() == ExtensionHookClass::Transform
        {
            let actions = (0..33)
                .map(|index| coverage_follow_up_action(&format!("request-{index}"), user_message("follow")))
                .collect();
            Ok(ExtensionActionBatch {
                decision: None,
                actions,
            })
        } else {
            Ok(empty_batch())
        }
    });
    let base = coverage_request_draft();
    let accepted = runtime
        .before_model_request(base.clone(), 16_384, CancellationToken::new())
        .await;
    coverage_assert_request_unchanged(accepted, &base);
}

// ── provider seams ─────────────────────────────────────────────────────────────────────

struct NonTransformRequestPort;

macro_rules! impl_non_transform_noop_domain {
    ($trait_name:ident, $method:ident) => {
        #[async_trait]
        impl $trait_name for NonTransformRequestPort {
            async fn $method(
                &self,
                _invocation: RuntimeExtensionInvocation,
            ) -> RawRuntimeExtensionResult {
                Ok(empty_batch())
            }
        }
    };
}

impl_non_transform_noop_domain!(RuntimeSessionExtensionPort, invoke_session);
impl_non_transform_noop_domain!(RuntimeRunExtensionPort, invoke_run);
impl_non_transform_noop_domain!(RuntimeMessageExtensionPort, invoke_message);
impl_non_transform_noop_domain!(RuntimeToolExtensionPort, invoke_tool);
impl_non_transform_noop_domain!(RuntimeCompactionExtensionPort, invoke_compaction);

#[async_trait]
impl RuntimeRequestExtensionPort for NonTransformRequestPort {
    fn has_request_hook(
        &self,
        _event: ExtensionLifecycleEvent,
        _class: ExtensionHookClass,
    ) -> bool {
        true
    }

    async fn invoke_request(
        &self,
        _invocation: RuntimeExtensionInvocation,
    ) -> RawRuntimeExtensionResult {
        Ok(empty_batch())
    }

    async fn dispatch_request(
        &self,
        _invocation: RuntimeExtensionInvocation,
    ) -> RuntimeExtensionResult {
        Ok(ValidatedRuntimeExtensionResult::Observe(ValidatedObserveResult))
    }
}

fn coverage_provider_runtime_with_port(
    port: Arc<dyn RuntimeExtensionPort>,
    cwd: &str,
) -> Arc<HarnessRuntimeExtensions> {
    Arc::new(HarnessRuntimeExtensions::new(
        port,
        "coverage-provider-session".into(),
        cwd.into(),
        false,
        None,
        ExtensionModelContextProjection::default(),
    ))
}

fn coverage_headers() -> ProviderRequestHeaders {
    ProviderRequestHeaders {
        format: ProviderWireFormat::OpenAiResponses,
        headers: [("x-base".into(), "base".into())]
            .into_iter()
            .collect(),
    }
}

fn coverage_payload() -> ProviderRequestPayload {
    ProviderRequestPayload {
        format: ProviderWireFormat::OpenAiResponses,
        payload: serde_json::json!({"model": "base"}),
    }
}

fn coverage_response() -> ProviderResponseMetadata {
    ProviderResponseMetadata {
        format: ProviderWireFormat::OpenAiResponses,
        status: 200,
        headers: [("content-type".into(), "text/event-stream".into())]
            .into_iter()
            .collect(),
    }
}

#[tokio::test]
async fn runtime_ext_provider_returns_original_request_when_no_hook_subscribed() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch())).with_request_hook(false)),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();

    let returned_headers = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap();
    assert_eq!(returned_headers.headers.get("x-base").map(String::as_str), Some("base"));

    let returned_payload = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap();
    assert_eq!(returned_payload.payload["model"], "base");
}

#[tokio::test]
async fn runtime_ext_provider_observe_no_hook_subscribed_is_noop() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch())).with_request_hook(false)),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();
    interceptor
        .interceptor()
        .observe_response(coverage_response())
        .await;
}

#[tokio::test]
async fn runtime_ext_provider_rejects_non_transform_result() {
    let runtime = coverage_provider_runtime_with_port(Arc::new(NonTransformRequestPort), "/workspace");
    let interceptor = runtime.provider_request_interceptor();

    let headers_error = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap_err();
    assert_eq!(headers_error.code, "contract_violation");

    let payload_error = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap_err();
    assert_eq!(payload_error.code, "contract_violation");
}

#[tokio::test]
async fn runtime_ext_provider_rejects_disallowed_transform_action() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|invocation| {
            if matches!(
                invocation.event(),
                ExtensionLifecycleEvent::BeforeProviderRequestHeaders
                    | ExtensionLifecycleEvent::BeforeProviderRequestRaw
            ) {
                Ok(ExtensionActionBatch {
                    decision: None,
                    actions: vec![coverage_follow_up_action(
                        "provider-follow-up",
                        user_message("follow"),
                    )],
                })
            } else {
                Ok(empty_batch())
            }
        })),
        "/workspace",
    );
    let interceptor = runtime.provider_request_interceptor();

    let headers_error = interceptor
        .interceptor()
        .transform_headers(coverage_headers())
        .await
        .unwrap_err();
    assert_eq!(headers_error.code, "invalid_provider_action");

    let payload_error = interceptor
        .interceptor()
        .transform_payload(coverage_payload())
        .await
        .unwrap_err();
    assert_eq!(payload_error.code, "invalid_provider_action");
}

#[tokio::test]
async fn runtime_ext_provider_observe_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch()))),
        "",
    );
    let interceptor = runtime.provider_request_interceptor();
    interceptor
        .interceptor()
        .observe_response(coverage_response())
        .await;
}

#[tokio::test]
async fn runtime_ext_provider_observe_request_failure_ignores_invocation_error_with_empty_cwd() {
    let runtime = coverage_provider_runtime_with_port(
        Arc::new(CoverageFnPort::new(|_| Ok(empty_batch()))),
        "",
    );
    let interceptor = runtime.provider_request_interceptor();
    interceptor
        .interceptor()
        .observe_request_failure(ProviderRequestFailure {
            format: ProviderWireFormat::AnthropicMessages,
            stage: ProviderRequestFailureStage::Transport,
            code: "tcp_connect".into(),
            message: "connection refused".into(),
        })
        .await;
}
