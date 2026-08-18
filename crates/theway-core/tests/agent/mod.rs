//! Tests for the bare `agent` runtime — split out of src
//! (see docs/rust-test-files.md).

use super::*;
use theway_llm_provider::{Message as PiMessage, UserContent, UserMessage, UserRole};

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

#[test]
fn pending_message_queue_all_drains_everything() {
    let mut q = PendingMessageQueue::new(QueueMode::All);
    q.enqueue(user_message("one"));
    q.enqueue(user_message("two"));

    let drained = q.drain();
    assert_eq!(drained.len(), 2);
    assert!(q.drain().is_empty());
}

#[test]
fn pending_message_queue_one_at_a_time_drains_oldest_only() {
    let mut q = PendingMessageQueue::new(QueueMode::OneAtATime);
    q.enqueue(user_message("one"));
    q.enqueue(user_message("two"));

    let first = q.drain();
    assert_eq!(first.len(), 1);
    let second = q.drain();
    assert_eq!(second.len(), 1);
    assert!(q.drain().is_empty());
}

#[test]
fn pending_message_queue_one_at_a_time_empty_is_noop() {
    let mut q = PendingMessageQueue::new(QueueMode::OneAtATime);
    assert!(q.drain().is_empty());
}

#[test]
fn guard_not_streaming_rejects_while_streaming() {
    let mut state = AgentState::default();
    state.is_streaming = true;
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        ..Default::default()
    });

    let err = agent.guard_not_streaming().unwrap_err();
    assert!(matches!(err, AgentRunError::AlreadyStreaming));

    agent.state().is_streaming = false;
    assert!(agent.guard_not_streaming().is_ok());
}

#[test]
fn subscribe_and_unsubscribe_remove_listener() {
    let agent = Agent::new(AgentOptions::default());
    let listener: LoopListener = Arc::new(move |_event, _cancel| Box::pin(async {}));

    let unsub = agent.subscribe(listener.clone());
    assert_eq!(agent.inner.await_listeners.lock().len(), 1);
    unsub();
    assert_eq!(agent.inner.await_listeners.lock().len(), 0);

    // Calling unsub twice is a no-op (the closure can only be called once,
    // so use a fresh subscription for idempotence verification).
    let unsub2 = agent.subscribe(listener.clone());
    assert_eq!(agent.inner.await_listeners.lock().len(), 1);
    unsub2();
    assert_eq!(agent.inner.await_listeners.lock().len(), 0);
}

#[test]
fn subscribe_sync_and_unsubscribe_remove_callback() {
    let agent = Agent::new(AgentOptions::default());
    let callback: LoopSyncCallback = Arc::new(move |_event| {});

    let unsub = agent.subscribe_sync(callback.clone());
    assert_eq!(agent.inner.sync_callbacks.lock().len(), 1);
    unsub();
    assert_eq!(agent.inner.sync_callbacks.lock().len(), 0);
}

#[test]
fn abort_and_interrupt_are_noops_without_active_tokens() {
    let agent = Agent::new(AgentOptions::default());
    agent.abort();
    agent.interrupt();
    assert!(agent.active_token().is_none());
}

#[test]
fn active_token_returns_clone_of_active_cancel() {
    let agent = Agent::new(AgentOptions::default());
    let token = tokio_util::sync::CancellationToken::new();
    *agent.inner.active_cancel.lock() = Some(token.clone());
    let active = agent.active_token().unwrap();
    active.cancel();
    assert!(token.is_cancelled());
}

#[test]
fn enqueue_steering_and_follow_up_push_into_queues() {
    let agent = Agent::new(AgentOptions::default());
    let msg = user_message("steer");
    agent.enqueue_steering(msg.clone());
    agent.enqueue_follow_up(msg.clone());

    assert_eq!(agent.inner.steering.lock().drain().len(), 1);
    assert_eq!(agent.inner.follow_up.lock().drain().len(), 1);
}

#[test]
fn convert_to_llm_uses_configured_callback_or_default() {
    let agent = Agent::new(AgentOptions::default());
    let msgs = vec![user_message("hi")];
    let out = agent.inner.convert_to_llm(&msgs);
    assert_eq!(out.len(), 1);

    let custom: crate::types::ConvertToLlm = Arc::new(|msgs| {
        msgs.iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(m) => Some(m.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect()
    });
    let agent = Agent::new(AgentOptions {
        convert_to_llm: Some(custom),
        ..Default::default()
    });
    let out = agent.inner.convert_to_llm(&msgs);
    assert_eq!(out.len(), 1);
}
