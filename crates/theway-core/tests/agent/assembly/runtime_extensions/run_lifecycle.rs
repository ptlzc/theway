//! End-to-end run lifecycle through `AgentHarness`: extension dispatch
//! ordering, follow-up queueing, and terminal event sequences.

use super::*;

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
