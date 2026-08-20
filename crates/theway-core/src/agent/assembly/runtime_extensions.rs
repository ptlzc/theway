mod compaction;
mod message;
mod tool;

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use theway_contract::extension::{
    ExtensionActionKind, ExtensionCommandOutcome, ExtensionErrorCode, ExtensionErrorEnvelope,
    ExtensionGateDecision, ExtensionHookClass, ExtensionLifecycleEvent, ExtensionModelRef,
    ExtensionScopeIds,
};
use tokio_util::sync::CancellationToken;

use crate::agent::runtime_extensions::{
    ExtensionModelContextProjection, RuntimeExtensionContext, RuntimeExtensionInvocation,
    RuntimeExtensionPort, RuntimeExtensionScopeAllocator, RuntimeExtensionScopeKind,
    ValidatedRuntimeExtensionResult,
};
use crate::types::AgentMessage;

#[derive(Default)]
struct ActiveScopes {
    run_id: Option<String>,
    turn_id: Option<String>,
    request_id: Option<String>,
    message_id: Option<String>,
    source_message_id: Option<String>,
}

const FOLLOW_UP_QUEUE_CAPACITY: usize = 32;
const FOLLOW_UP_DEDUP_CAPACITY: usize = 256;

#[derive(Default)]
struct FollowUpQueue {
    queued: VecDeque<(String, AgentMessage)>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
}

tokio::task_local! {
    static RUNTIME_EXTENSION_DISPATCH: ();
}

pub(super) struct HarnessRuntimeExtensions {
    port: Arc<dyn RuntimeExtensionPort>,
    scopes: RuntimeExtensionScopeAllocator,
    cwd: String,
    has_interactive_client: bool,
    model: Mutex<Option<ExtensionModelRef>>,
    active: Mutex<ActiveScopes>,
    session_started: AtomicBool,
    session_shutdown: AtomicBool,
    follow_ups: Mutex<FollowUpQueue>,
    model_context: ExtensionModelContextProjection,
}

impl HarnessRuntimeExtensions {
    pub(super) fn new(
        port: Arc<dyn RuntimeExtensionPort>,
        session_id: String,
        cwd: String,
        has_interactive_client: bool,
        model: Option<ExtensionModelRef>,
        model_context: ExtensionModelContextProjection,
    ) -> Self {
        let scopes = RuntimeExtensionScopeAllocator::new(session_id)
            .expect("runtime extension session id is normalized by harness construction");
        Self {
            port,
            scopes,
            cwd,
            has_interactive_client,
            model: Mutex::new(model),
            active: Mutex::new(ActiveScopes::default()),
            session_started: AtomicBool::new(false),
            session_shutdown: AtomicBool::new(false),
            follow_ups: Mutex::new(FollowUpQueue::default()),
            model_context,
        }
    }

    pub(super) fn port(&self) -> &Arc<dyn RuntimeExtensionPort> {
        &self.port
    }

    pub(super) fn compaction_context_messages(&self) -> Vec<AgentMessage> {
        self.model_context.compaction_messages()
    }

    pub(super) fn set_model(&self, model: &theway_llm_provider::Model) {
        *self.model.lock() = Some(ExtensionModelRef {
            provider: model.provider.0.clone(),
            model: model.id.clone(),
        });
    }

    pub(super) fn begin_run(&self) -> Result<(), ExtensionErrorEnvelope> {
        let mut active = self.active.lock();
        active.run_id = Some(self.allocate(RuntimeExtensionScopeKind::Run)?);
        active.turn_id = None;
        active.request_id = None;
        Ok(())
    }

    pub(super) fn begin_turn(&self) -> Result<(), ExtensionErrorEnvelope> {
        let mut active = self.active.lock();
        active.turn_id = Some(self.allocate(RuntimeExtensionScopeKind::Turn)?);
        active.request_id = Some(self.allocate(RuntimeExtensionScopeKind::Request)?);
        active.message_id = None;
        active.source_message_id = None;
        Ok(())
    }

    pub(super) fn settle_run(&self) {
        *self.active.lock() = ActiveScopes::default();
    }

    pub(super) fn reject_reentrant_operation(&self) -> Result<(), ExtensionErrorEnvelope> {
        if RUNTIME_EXTENSION_DISPATCH.try_with(|()| ()).is_ok() {
            Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::ReentrantCall,
                "runtime operations cannot be started synchronously from a lifecycle hook",
            ))
        } else {
            Ok(())
        }
    }

    pub(super) fn take_follow_up(&self) -> Option<AgentMessage> {
        self.follow_ups
            .lock()
            .queued
            .pop_front()
            .map(|(_, message)| message)
    }

    pub(super) async fn ensure_session_start(&self) {
        if self.session_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::SessionStart,
            ExtensionHookClass::Observe,
            serde_json::json!({"reason": "initial"}),
            false,
        ) else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_session(invocation)).await;
    }

    pub(super) async fn shutdown(&self) {
        if self.session_shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::SessionShutdown,
            ExtensionHookClass::Observe,
            serde_json::json!({}),
            false,
        ) else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_session(invocation)).await;
    }

    pub(super) async fn transform_input(&self, message: AgentMessage) -> InputTransformOutcome {
        let payload = serde_json::json!({"message": message});
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::Input,
            ExtensionHookClass::Transform,
            payload,
            false,
        ) else {
            return InputTransformOutcome::Run(message);
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_request(invocation)).await
        else {
            return InputTransformOutcome::Run(message);
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::ReplaceInput
                    | ExtensionActionKind::EmitCommandOutcome
                    | ExtensionActionKind::EnqueueFollowUp
            )
        }) {
            return InputTransformOutcome::Run(message);
        }

        let original = message.clone();
        let mut transformed = message;
        let mut outcome = None;
        let mut follow_ups = Vec::new();
        for action in result.actions() {
            match action.kind {
                ExtensionActionKind::ReplaceInput => {
                    let Some(value) = action.payload.get("message") else {
                        return InputTransformOutcome::Run(original);
                    };
                    let Ok(replacement) = serde_json::from_value::<AgentMessage>(value.clone())
                    else {
                        return InputTransformOutcome::Run(original);
                    };
                    if !same_message_role(&transformed, &replacement) {
                        return InputTransformOutcome::Run(original);
                    }
                    transformed = replacement;
                }
                ExtensionActionKind::EmitCommandOutcome => {
                    let Ok(parsed) = serde_json::from_value(action.payload.clone()) else {
                        return InputTransformOutcome::Run(original);
                    };
                    outcome = Some(parsed);
                }
                ExtensionActionKind::EnqueueFollowUp => {
                    let Some(parsed) = parse_follow_up(&action.payload) else {
                        return InputTransformOutcome::Run(original);
                    };
                    follow_ups.push(parsed);
                }
                _ => unreachable!("checked action allowlist"),
            }
        }
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return InputTransformOutcome::Run(original);
        }
        match outcome {
            Some(outcome) => InputTransformOutcome::Handled(outcome),
            None => InputTransformOutcome::Run(transformed),
        }
    }

    pub(super) async fn transform_context(
        &self,
        messages: Vec<AgentMessage>,
        cancel: CancellationToken,
    ) -> Vec<AgentMessage> {
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::Context,
            ExtensionHookClass::Transform,
            serde_json::json!({"messages": messages}),
            cancel.is_cancelled(),
        ) else {
            return messages;
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_request(invocation)).await
        else {
            return messages;
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::ReplaceContext | ExtensionActionKind::EnqueueFollowUp
            )
        }) {
            return messages;
        }
        let replacement = result
            .actions()
            .iter()
            .find(|action| action.kind == ExtensionActionKind::ReplaceContext)
            .and_then(|action| action.payload.get("messages"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_else(|| messages.clone());
        let follow_ups = result
            .actions()
            .iter()
            .filter(|action| action.kind == ExtensionActionKind::EnqueueFollowUp)
            .map(|action| parse_follow_up(&action.payload))
            .collect::<Option<Vec<_>>>();
        let Some(follow_ups) = follow_ups else {
            return messages;
        };
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return messages;
        }
        replacement
    }

    pub(super) async fn before_model_selection(
        &self,
        target: &theway_llm_provider::Model,
    ) -> Result<(), ExtensionErrorEnvelope> {
        let invocation = self.invocation(
            ExtensionLifecycleEvent::BeforeModelSelection,
            ExtensionHookClass::Gate,
            serde_json::json!({
                "provider": target.provider.0,
                "model": target.id,
            }),
            false,
        )?;
        let result = self.guarded(self.port.dispatch_request(invocation)).await?;
        gate_allows(result)
    }

    pub(super) async fn model_selected(&self, model: &theway_llm_provider::Model) {
        self.set_model(model);
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::ModelSelected,
            ExtensionHookClass::Observe,
            serde_json::json!({
                "provider": model.provider.0,
                "model": model.id,
            }),
            false,
        ) else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_request(invocation)).await;
    }

    pub(super) async fn gate_session_operation(
        &self,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) -> Result<(), ExtensionErrorEnvelope> {
        let invocation = self.invocation(event, ExtensionHookClass::Gate, payload, false)?;
        let result = self.guarded(self.port.dispatch_session(invocation)).await?;
        gate_allows(result)
    }

    pub(super) async fn observe_session_operation(
        &self,
        event: ExtensionLifecycleEvent,
        payload: Value,
    ) {
        let Ok(invocation) = self.invocation(event, ExtensionHookClass::Observe, payload, false)
        else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_session(invocation)).await;
    }

    pub(super) async fn observe_run(
        &self,
        event: ExtensionLifecycleEvent,
        payload: Value,
        cancelled: bool,
    ) {
        let Ok(invocation) =
            self.invocation(event, ExtensionHookClass::Observe, payload, cancelled)
        else {
            return;
        };
        let _ = self.guarded(self.port.dispatch_run(invocation)).await;
    }

    pub(super) async fn before_run(&self) -> BeforeRunPatch {
        let Ok(invocation) = self.invocation(
            ExtensionLifecycleEvent::BeforeRun,
            ExtensionHookClass::Transform,
            serde_json::json!({}),
            false,
        ) else {
            return BeforeRunPatch::default();
        };
        let Ok(ValidatedRuntimeExtensionResult::Transform(result)) =
            self.guarded(self.port.dispatch_run(invocation)).await
        else {
            return BeforeRunPatch::default();
        };
        if result.actions().iter().any(|action| {
            !matches!(
                action.kind,
                ExtensionActionKind::PatchRunContext | ExtensionActionKind::EnqueueFollowUp
            )
        }) {
            return BeforeRunPatch::default();
        }
        let patch = result
            .actions()
            .iter()
            .find(|action| action.kind == ExtensionActionKind::PatchRunContext)
            .map(|action| serde_json::from_value::<BeforeRunPatch>(action.payload.clone()))
            .transpose();
        let Ok(patch) = patch else {
            return BeforeRunPatch::default();
        };
        let follow_ups = result
            .actions()
            .iter()
            .filter(|action| action.kind == ExtensionActionKind::EnqueueFollowUp)
            .map(|action| parse_follow_up(&action.payload))
            .collect::<Option<Vec<_>>>();
        let Some(follow_ups) = follow_ups else {
            return BeforeRunPatch::default();
        };
        if self.enqueue_follow_ups(follow_ups).is_err() {
            return BeforeRunPatch::default();
        }
        patch.unwrap_or_default()
    }

    pub(super) fn make_loop_listener(
        self: &Arc<Self>,
    ) -> (crate::agent::LoopListener, Arc<AtomicBool>) {
        let runtime = Arc::clone(self);
        let cancelled = Arc::new(AtomicBool::new(false));
        let listener_cancelled = Arc::clone(&cancelled);
        let listener: crate::agent::LoopListener = Arc::new(move |event, cancel| {
            let runtime = Arc::clone(&runtime);
            let cancelled = Arc::clone(&listener_cancelled);
            Box::pin(async move {
                match event {
                    crate::types::LoopEvent::RunStarted => {
                        runtime
                            .observe_run(
                                ExtensionLifecycleEvent::RunStarted,
                                serde_json::json!({}),
                                cancel.is_cancelled(),
                            )
                            .await;
                    }
                    crate::types::LoopEvent::TurnStart => {
                        if runtime.begin_turn().is_ok() {
                            runtime
                                .observe_run(
                                    ExtensionLifecycleEvent::TurnStarted,
                                    serde_json::json!({}),
                                    cancel.is_cancelled(),
                                )
                                .await;
                        }
                    }
                    crate::types::LoopEvent::TurnCompleted {
                        message,
                        tool_results,
                    } => {
                        runtime
                            .observe_run(
                                ExtensionLifecycleEvent::TurnCompleted,
                                serde_json::json!({
                                    "message": message,
                                    "toolResults": tool_results,
                                }),
                                cancel.is_cancelled(),
                            )
                            .await;
                    }
                    crate::types::LoopEvent::MessageStart { message } => {
                        runtime.observe_message_start(&message, &cancel).await;
                    }
                    crate::types::LoopEvent::MessageUpdate {
                        message,
                        assistant_message_event,
                    } => {
                        runtime
                            .observe_message_update(&message, &assistant_message_event, &cancel)
                            .await;
                    }
                    crate::types::LoopEvent::MessageEnd { message } => {
                        runtime.complete_message(&message);
                    }
                    crate::types::LoopEvent::ToolExecutionStart {
                        tool_call_id,
                        tool_name,
                        args,
                    } => {
                        runtime
                            .observe_tool_execution(
                                ExtensionLifecycleEvent::ToolExecutionStart,
                                tool_call_id,
                                serde_json::json!({"toolName": tool_name, "args": args}),
                                &cancel,
                            )
                            .await;
                    }
                    crate::types::LoopEvent::ToolExecutionUpdate {
                        tool_call_id,
                        tool_name,
                        args,
                        partial_result,
                    } => {
                        runtime
                            .observe_tool_execution(
                                ExtensionLifecycleEvent::ToolExecutionUpdate,
                                tool_call_id,
                                serde_json::json!({
                                    "toolName": tool_name,
                                    "args": args,
                                    "partialResult": partial_result,
                                }),
                                &cancel,
                            )
                            .await;
                    }
                    crate::types::LoopEvent::ToolExecutionEnd {
                        tool_call_id,
                        tool_name,
                        result,
                        is_error,
                    } => {
                        runtime
                            .observe_tool_execution(
                                ExtensionLifecycleEvent::ToolExecutionEnd,
                                tool_call_id,
                                serde_json::json!({
                                    "toolName": tool_name,
                                    "result": result,
                                    "isError": is_error,
                                }),
                                &cancel,
                            )
                            .await;
                    }
                    crate::types::LoopEvent::RunEnded { .. } => {
                        cancelled.store(cancel.is_cancelled(), Ordering::Release);
                    }
                    _ => {}
                }
            })
        });
        (listener, cancelled)
    }

    fn invocation(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
        payload: Value,
        cancelled: bool,
    ) -> Result<RuntimeExtensionInvocation, ExtensionErrorEnvelope> {
        self.invocation_scoped(event, class, payload, cancelled, None, None)
    }

    fn invocation_scoped(
        &self,
        event: ExtensionLifecycleEvent,
        class: ExtensionHookClass,
        payload: Value,
        cancelled: bool,
        message_id: Option<String>,
        tool_call_id: Option<String>,
    ) -> Result<RuntimeExtensionInvocation, ExtensionErrorEnvelope> {
        let sequence = self.scopes.next_sequence().map_err(scope_error)?;
        let active = self.active.lock();
        let mut context =
            RuntimeExtensionContext::new(self.scopes.session_id(), self.cwd.clone(), sequence);
        context.scope = ExtensionScopeIds {
            run_id: active.run_id.clone(),
            turn_id: active.turn_id.clone(),
            request_id: active.request_id.clone(),
            message_id: message_id.or_else(|| active.message_id.clone()),
            tool_call_id,
        };
        drop(active);
        context.model = self.model.lock().clone();
        context.has_interactive_client = self.has_interactive_client;
        context.cancelled = cancelled;
        RuntimeExtensionInvocation::new(event, class, context, payload)
    }

    fn allocate(&self, kind: RuntimeExtensionScopeKind) -> Result<String, ExtensionErrorEnvelope> {
        self.scopes.allocate(kind).map_err(scope_error)
    }

    async fn guarded<T>(
        &self,
        future: impl Future<Output = Result<T, ExtensionErrorEnvelope>>,
    ) -> Result<T, ExtensionErrorEnvelope> {
        if RUNTIME_EXTENSION_DISPATCH.try_with(|()| ()).is_ok() {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::ReentrantCall,
                "recursive runtime extension lifecycle dispatch is not allowed",
            ));
        }
        RUNTIME_EXTENSION_DISPATCH.scope((), future).await
    }

    fn enqueue_follow_ups(
        &self,
        follow_ups: Vec<(String, AgentMessage)>,
    ) -> Result<(), ExtensionErrorEnvelope> {
        let mut queue = self.follow_ups.lock();
        let new_count = follow_ups
            .iter()
            .filter(|(id, _)| !queue.seen.contains(id))
            .count();
        if queue.queued.len().saturating_add(new_count) > FOLLOW_UP_QUEUE_CAPACITY {
            return Err(ExtensionErrorEnvelope::new(
                ExtensionErrorCode::ResourceLimit,
                "runtime extension follow-up queue capacity exceeded",
            ));
        }
        for (id, message) in follow_ups {
            if queue.seen.insert(id.clone()) {
                queue.seen_order.push_back(id.clone());
                queue.queued.push_back((id, message));
                while queue.seen_order.len() > FOLLOW_UP_DEDUP_CAPACITY {
                    if let Some(expired) = queue.seen_order.pop_front() {
                        queue.seen.remove(&expired);
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) enum InputTransformOutcome {
    Run(AgentMessage),
    Handled(ExtensionCommandOutcome),
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BeforeRunPatch {
    #[serde(default)]
    pub(super) system_prompt: Option<String>,
    #[serde(default)]
    pub(super) messages: Vec<AgentMessage>,
}

fn same_message_role(left: &AgentMessage, right: &AgentMessage) -> bool {
    matches!(
        (left, right),
        (
            AgentMessage::Llm(theway_llm_provider::Message::User(_)),
            AgentMessage::Llm(theway_llm_provider::Message::User(_))
        ) | (
            AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)),
            AgentMessage::Llm(theway_llm_provider::Message::Assistant(_))
        ) | (
            AgentMessage::Llm(theway_llm_provider::Message::ToolResult(_)),
            AgentMessage::Llm(theway_llm_provider::Message::ToolResult(_))
        ) | (AgentMessage::Custom(_), AgentMessage::Custom(_))
    )
}

fn gate_allows(result: ValidatedRuntimeExtensionResult) -> Result<(), ExtensionErrorEnvelope> {
    let ValidatedRuntimeExtensionResult::Gate(result) = result else {
        return Err(ExtensionErrorEnvelope::new(
            ExtensionErrorCode::ContractViolation,
            "core gate dispatch returned a non-gate result",
        ));
    };
    match result.decision() {
        ExtensionGateDecision::Abstain | ExtensionGateDecision::Allow => Ok(()),
        ExtensionGateDecision::Deny { code, message }
        | ExtensionGateDecision::Cancel { code, message } => Err(ExtensionErrorEnvelope::new(
            ExtensionErrorCode::Cancelled,
            format!("{code}: {message}"),
        )),
    }
}

fn scope_error(error: impl std::fmt::Display) -> ExtensionErrorEnvelope {
    ExtensionErrorEnvelope::new(ExtensionErrorCode::ResourceLimit, error.to_string())
}

fn parse_follow_up(payload: &Value) -> Option<(String, AgentMessage)> {
    let id = payload.get("followUpId")?.as_str()?.trim();
    if id.is_empty() || id.len() > 128 {
        return None;
    }
    let message: AgentMessage = serde_json::from_value(payload.get("message")?.clone()).ok()?;
    if !matches!(
        message,
        AgentMessage::Llm(theway_llm_provider::Message::User(_))
    ) {
        return None;
    }
    Some((id.to_string(), message))
}
