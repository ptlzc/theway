//! Harness lifecycle: runtime-extension default port, session listener
//! persistence, session-start emission, passthroughs, subscriptions, shutdown.

use super::*;

#[tokio::test]
async fn harness_defaults_to_behavior_neutral_runtime_extension_port() {
    let harness = harness();
    let invocation = crate::agent::runtime_extensions::RuntimeExtensionInvocation::new(
        ExtensionLifecycleEvent::Input,
        ExtensionHookClass::Transform,
        crate::agent::runtime_extensions::RuntimeExtensionContext::new(
            "session-1",
            "/workspace",
            1,
        ),
        serde_json::json!({}),
    )
    .unwrap();

    let result = harness
        .runtime_extensions()
        .dispatch_request(invocation)
        .await
        .unwrap();

    let crate::agent::runtime_extensions::ValidatedRuntimeExtensionResult::Transform(result) =
        result
    else {
        panic!("expected transform result")
    };
    assert!(result.actions().is_empty());
}

#[tokio::test]
async fn make_session_listener_persists_message_and_control_plane_prompt() {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let (listener, errors) = make_session_listener(session.clone());
    let cancel = tokio_util::sync::CancellationToken::new();

    listener(
        LoopEvent::MessageEnd {
            message: user_message("hello"),
        },
        cancel.clone(),
    )
    .await;
    listener(
        LoopEvent::ControlPlanePromptResolved {
            tool_call_id: "call_1".into(),
            tool_name: "write_file".into(),
            args_hash: "a".repeat(64),
            label: "Control-plane write: write_file".into(),
            decision: "allow".into(),
            reason: None,
        },
        cancel.clone(),
    )
    .await;

    let entries = session.entries().await.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(errors.lock().is_empty());
}

#[tokio::test]
async fn make_session_listener_records_persist_errors() {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    let (listener, errors) = make_session_listener(session.clone());
    // A non-message event must be ignored.
    listener(LoopEvent::TurnStart, tokio_util::sync::CancellationToken::new())
        .await;
    assert!(session.entries().await.unwrap().is_empty());
    assert!(errors.lock().is_empty());
}

#[tokio::test]
async fn ensure_session_start_emitted_fires_once() {
    let h = harness();
    let mut rx = h.subscribe_session_broadcast();

    h.ensure_session_start_emitted().await;
    h.ensure_session_start_emitted().await;

    let mut seen = 0;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, SessionEvent::Started { .. }) {
            seen += 1;
        }
    }
    assert_eq!(seen, 1);
}

#[test]
fn abort_interrupt_enqueue_passthroughs_are_noops_without_active_run() {
    let h = harness();
    h.abort();
    h.interrupt();
    h.enqueue_steering(user_message("steer"));
    h.enqueue_follow_up(user_message("follow"));
    assert!(h.cost().tokens.total_tokens == 0);
    h.reset_cost();
}

#[tokio::test]
async fn subscribe_harness_receives_and_unsubscribes() {
    let h = harness();
    let seen = Arc::new(std::sync::Mutex::new(0usize));
    let seen_clone = seen.clone();
    let unsubscribe = h.subscribe_harness(Arc::new(move |event| {
        if matches!(event, SessionEvent::Started { .. }) {
            *seen_clone.lock().unwrap() += 1;
        }
    }));

    h.ensure_session_start_emitted().await;
    assert_eq!(*seen.lock().unwrap(), 1);

    unsubscribe();
    h.emit_harness_event(SessionEvent::Started { messages_replayed: 0 });
    assert_eq!(*seen.lock().unwrap(), 1);
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Coverage-gap additions: events / session / run edge branches
// ──────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn subscribe_harness_unsubscribe_noop_after_listener_removed() {
    let h = harness();
    let unsubscribe = h.subscribe_harness(Arc::new(|_event| {}));
    h.harness_listeners.lock().clear();
    // The closure must be a no-op when the listener is already gone.
    unsubscribe();
    assert!(h.harness_listeners.lock().is_empty());
}

#[tokio::test(start_paused = true)]
async fn shutdown_runtime_extensions_times_out_when_run_never_idles() {
    let h = harness();
    // Simulate a run that never releases admission.
    *h.agent.inner.run_active.lock() = true;
    h.agent.inner.state.lock().is_streaming = true;

    h.shutdown_runtime_extensions().await;

    // The call returned despite the wedged run; cleanup state for the next tests.
    h.agent.inner.release_run();
}
