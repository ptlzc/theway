use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use theway_contract::extension::{
    ExtensionAbiMajor, ExtensionAction, ExtensionActionBatch, ExtensionActionKind,
    ExtensionGateDecision, ExtensionHookClass, ExtensionLifecycleEvent,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ErrorReason, StopReason, Usage,
};

use super::*;
use crate::agent::runtime_extensions::{
    RawRuntimeExtensionResult, RuntimeCompactionExtensionPort, RuntimeExtensionInvocation,
    RuntimeMessageExtensionPort, RuntimeRequestExtensionPort, RuntimeRunExtensionPort,
    RuntimeSessionExtensionPort, RuntimeToolExtensionPort,
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
                abi_major: ExtensionAbiMajor::V2,
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
        abi_major: ExtensionAbiMajor::V2,
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
            abi_major: ExtensionAbiMajor::V2,
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
            abi_major: ExtensionAbiMajor::V2,
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
            abi_major: ExtensionAbiMajor::V2,
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
            abi_major: ExtensionAbiMajor::V2,
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
            abi_major: ExtensionAbiMajor::V2,
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
                abi_major: ExtensionAbiMajor::V2,
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
