//! Tests for `agent::run_loop::utils` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::Arc;

use super::*;
use crate::agent::{Agent, AgentInner, LoopListener};
use crate::types::{AgentContext, AgentLoopTurnUpdate, AgentMessage, LoopEvent, ThinkingLevel};
use theway_llm_provider::{Model, UserContent, UserMessage, UserRole};

fn faux_model() -> Model {
    Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

fn test_inner() -> Arc<AgentInner> {
    let agent = Agent::new(Default::default());
    agent.inner.clone()
}

#[test]
fn apply_turn_update_replaces_context_model_and_thinking_level() {
    let inner = test_inner();
    let mut state = inner.state.lock();
    state.system_prompt = "old".into();
    state.messages = vec![user_message("old")];
    state.model = None;
    state.thinking_level = None;
    drop(state);

    let ctx = AgentContext {
        system_prompt: "new".into(),
        messages: vec![user_message("new")],
        tools: Vec::new(),
    };
    apply_turn_update(
        &inner,
        AgentLoopTurnUpdate {
            context: Some(ctx),
            model: Some(faux_model()),
            thinking_level: Some(ThinkingLevel::High),
        },
    );

    let g = inner.state.lock();
    assert_eq!(g.system_prompt, "new");
    assert_eq!(g.messages.len(), 1);
    assert_eq!(g.thinking_level, Some(ThinkingLevel::High));
    assert_eq!(g.model.as_ref().map(|m| m.id.as_str()), Some("faux"));
}

#[test]
fn apply_turn_update_partial_update_keeps_other_fields() {
    let inner = test_inner();
    inner.state.lock().thinking_level = Some(ThinkingLevel::Low);

    apply_turn_update(
        &inner,
        AgentLoopTurnUpdate {
            model: Some(faux_model()),
            ..Default::default()
        },
    );

    let g = inner.state.lock();
    assert_eq!(g.thinking_level, Some(ThinkingLevel::Low));
    assert_eq!(g.model.as_ref().map(|m| m.id.as_str()), Some("faux"));
}

#[test]
fn snapshot_context_captures_system_prompt_messages_and_tools() {
    let inner = test_inner();
    inner.state.lock().system_prompt = "sys".into();
    inner.state.lock().messages = vec![user_message("hi")];

    let ctx = snapshot_context(&inner);

    assert_eq!(ctx.system_prompt, "sys");
    assert_eq!(ctx.messages.len(), 1);
    assert!(ctx.tools.is_empty());
}

#[tokio::test]
async fn emit_dispatches_sync_await_and_broadcast_segments_in_order() {
    let inner = test_inner();
    let cancel = tokio_util::sync::CancellationToken::new();

    let sync_seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sync_clone = sync_seen.clone();
    inner
        .sync_callbacks
        .lock()
        .push(Arc::new(move |ev: &LoopEvent| {
            sync_clone.lock().unwrap().push(match ev {
                LoopEvent::TurnStart => "sync-turn-start".into(),
                _ => "sync-other".into(),
            });
        }));

    let await_seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let await_clone = await_seen.clone();
    let listener: LoopListener = Arc::new(move |ev: LoopEvent, _cancel| {
        let await_clone = await_clone.clone();
        Box::pin(async move {
            await_clone.lock().unwrap().push(match ev {
                LoopEvent::TurnStart => "await-turn-start".into(),
                _ => "await-other".into(),
            });
        })
    });
    inner.await_listeners.lock().push(listener);

    let mut rx = inner.broadcast_tx.subscribe();

    emit(&inner, LoopEvent::TurnStart, &cancel).await;

    assert_eq!(sync_seen.lock().unwrap().as_slice(), ["sync-turn-start"]);
    assert_eq!(await_seen.lock().unwrap().as_slice(), ["await-turn-start"]);
    let broadcast_event = rx.try_recv().expect("broadcast event");
    assert!(matches!(broadcast_event, LoopEvent::TurnStart));
}

#[tokio::test]
async fn emit_isolates_panicking_sync_callback() {
    let inner = test_inner();
    let cancel = tokio_util::sync::CancellationToken::new();

    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_clone = seen.clone();
    inner
        .sync_callbacks
        .lock()
        .push(Arc::new(move |_: &LoopEvent| {
            panic!("boom");
        }));
    inner
        .sync_callbacks
        .lock()
        .push(Arc::new(move |ev: &LoopEvent| {
            if matches!(ev, LoopEvent::RunStarted) {
                seen_clone.lock().unwrap().push("survived".into());
            }
        }));

    emit(&inner, LoopEvent::RunStarted, &cancel).await;

    assert_eq!(seen.lock().unwrap().as_slice(), ["survived"]);
}

#[tokio::test]
async fn finalize_emits_run_ended_and_resets_streaming_cancel_tokens() {
    let inner = test_inner();
    inner.state.lock().messages = vec![user_message("done")];
    inner.state.lock().is_streaming = true;
    *inner.active_cancel.lock() = Some(tokio_util::sync::CancellationToken::new());
    *inner.turn_cancel.lock() = Some(tokio_util::sync::CancellationToken::new());
    let mut rx = inner.broadcast_tx.subscribe();

    finalize(
        &inner,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(!inner.state.lock().is_streaming);
    assert!(inner.active_cancel.lock().is_none());
    assert!(inner.turn_cancel.lock().is_none());
    let event = rx.try_recv().expect("RunEnded broadcast");
    match event {
        LoopEvent::RunEnded { messages } => assert_eq!(messages.len(), 1),
        _ => panic!("expected RunEnded"),
    }
}

#[test]
fn compute_args_hash_is_stable_and_sorts_object_keys() {
    let a = serde_json::json!({"b": 1, "a": [1, 2]});
    let b = serde_json::json!({"a": [1, 2], "b": 1});
    let c = serde_json::json!({"a": [1, 3], "b": 1});

    let ha = compute_args_hash(&a);
    let hb = compute_args_hash(&b);
    let hc = compute_args_hash(&c);

    assert_eq!(ha, hb);
    assert_ne!(ha, hc);
    assert_eq!(ha.len(), 64);
    assert!(ha.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn default_prompt_payload_redacts_values_and_bounds_keys() {
    let payload = default_prompt_payload(
        "write_file",
        &serde_json::json!({
            "path": "/tmp/secret-url",
            "content": "secret-body",
            "b-key": 1,
            "a-key": 2,
        }),
    );

    assert_eq!(payload["tool_name"], serde_json::json!("write_file"));
    let keys = payload["args_keys"].as_array().unwrap();
    assert_eq!(keys, &vec![
        serde_json::json!("a-key"),
        serde_json::json!("b-key"),
        serde_json::json!("content"),
        serde_json::json!("path"),
    ]);
    // Raw values must not leak into the default payload.
    assert!(payload.get("path").is_none());
    assert!(payload.get("content").is_none());
    assert_eq!(payload["args_hash"].as_str().unwrap().len(), 64);
}

#[test]
fn default_prompt_payload_truncates_long_keys_to_64_chars() {
    let long_key = "k".repeat(70);
    let payload = default_prompt_payload("t", &serde_json::json!({ long_key.clone(): 1 }));

    let keys = payload["args_keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].as_str().unwrap().chars().count(), 65);
    assert!(keys[0].as_str().unwrap().ends_with('…'));
}

#[test]
fn default_prompt_payload_returns_empty_keys_for_non_object_args() {
    let payload = default_prompt_payload("t", &serde_json::json!([1, 2, 3]));
    assert_eq!(payload["args_keys"].as_array().unwrap().len(), 0);
    assert_eq!(payload["args_hash"].as_str().unwrap().len(), 64);
}
