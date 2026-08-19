//! Goal lifecycle fallback behavior.

use std::sync::Arc;

use super::super::*;
use crate::agent::assembly::{AgentHarness, AgentHarnessOptions};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::{Session, SessionStorage};
use crate::multiagent::graph::engine::DagEngine;
use crate::multiagent::graph::types::DagStatus;
use crate::multiagent::registry::AgentJobRegistry;
use crate::multiagent::types::AgentRunParams;
use theway_llm_provider::{Message as PiMessage, UserContent, UserMessage, UserRole};

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
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

fn harness() -> Arc<AgentHarness> {
    let storage: Arc<dyn SessionStorage> = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(),
        session,
    )))
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

#[test]
fn goal_tick_handles_some_and_none_run_ids() {
    let engine = DagEngine::new();
    let run_id = engine.plan_goal("tick", None);

    goal_tick(&engine, Some(&run_id), 1, false, Some("not yet"));

    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.node("main").unwrap().attempt, 1);
    assert_eq!(run.node("main").unwrap().error.as_deref(), Some("not yet"));

    goal_tick(&engine, Some(&run_id), 2, true, None);
    assert_eq!(engine.get_run(&run_id).unwrap().status, DagStatus::Completed);

    goal_tick(&engine, None, 3, false, None);
}

#[test]
fn complete_goal_cancelled_handles_some_and_none_run_ids() {
    let engine = DagEngine::new();
    let run_id = engine.plan_goal("complete", None);

    complete_goal_cancelled(&engine, Some(&run_id), "cancelled");

    let run = engine.get_run(&run_id).unwrap();
    assert_eq!(run.status, DagStatus::Cancelled);
    assert_eq!(run.error.as_deref(), Some("cancelled"));

    complete_goal_cancelled(&engine, None, "noop");
}

#[tokio::test]
async fn session_id_from_harness_reads_storage_metadata() {
    let h = harness();
    let id = session_id_from_harness(&h).await.unwrap();
    assert!(!id.is_empty());
}

#[tokio::test]
async fn stop_hook_returns_pause_when_harness_cell_unset() {
    let harness_cell = Arc::new(std::sync::OnceLock::new());
    let hook = stop_hook(
        harness_cell,
        Arc::new(DagEngine::new()),
        Arc::new(|_| None),
        AgentJobRegistry::new(),
        None,
    );
    let ctx = OnTurnEndContext {
        transcript: vec![user_msg("hi")],
        continuation_count: 0,
        last_user_prompt: Some("hi".into()),
    };
    let decision = hook(ctx, tokio_util::sync::CancellationToken::new()).await;

    assert!(matches!(
        decision.action,
        TurnEndAction::Pause { ref reason } if reason == "goal hook was not initialized"
    ));
}

#[tokio::test]
async fn evaluate_stop_hook_returns_noop_without_goal() {
    let h = harness();
    let engine = Arc::new(DagEngine::new());
    let ctx = OnTurnEndContext {
        transcript: vec![user_msg("hi")],
        continuation_count: 0,
        last_user_prompt: Some("hi".into()),
    };
    let decision = evaluate_stop_hook(
        h.clone(),
        engine,
        Arc::new(|_| None),
        AgentJobRegistry::new(),
        None,
        ctx,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(matches!(decision.action, TurnEndAction::Noop));
}

#[tokio::test]
async fn evaluate_stop_hook_pauses_when_no_model() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();
    h.agent().state().model = None;

    let engine = Arc::new(DagEngine::new());
    let ctx = OnTurnEndContext {
        transcript: vec![user_msg("hi")],
        continuation_count: 0,
        last_user_prompt: Some("hi".into()),
    };
    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        Arc::new(|_| None),
        AgentJobRegistry::new(),
        None,
        ctx,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(matches!(
        decision.action,
        TurnEndAction::Pause { ref reason } if reason == "goal evaluator has no current model"
    ));
    let current = current(&h).await.unwrap();
    assert_eq!(current.status, GoalStatus::Paused);
}

#[tokio::test]
async fn evaluate_stop_hook_pauses_when_no_evaluator_spec() {
    let h = harness();
    set(&h, "finish".into()).await.unwrap();

    let engine = Arc::new(DagEngine::new());
    let ctx = OnTurnEndContext {
        transcript: vec![user_msg("hi")],
        continuation_count: 0,
        last_user_prompt: Some("hi".into()),
    };
    let decision = evaluate_stop_hook(
        h.clone(),
        engine.clone(),
        Arc::new(|_| None),
        AgentJobRegistry::new(),
        None,
        ctx,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(matches!(
        decision.action,
        TurnEndAction::Pause { ref reason } if reason.contains("no goal-evaluator spec registered")
    ));
}

#[test]
fn evaluator_system_prompt_contains_contract() {
    let prompt = evaluator_system_prompt();
    assert!(prompt.contains("ok"));
    assert!(prompt.contains("reason"));
}

#[test]
fn parse_decision_falls_back_to_embedded_json_object() {
    let decision = parse_decision("prefix {\"ok\": true, \"reason\": \"done\"} suffix").unwrap();
    assert!(decision.ok);
    assert_eq!(decision.reason, "done");
}

#[test]
fn agent_run_params_are_send_sync() {
    // Compile-time sanity: AgentRunResolver boxes an Fn returning AgentRunParams.
    let resolver: AgentRunResolver = Arc::new(|_name| {
        Some(AgentRunParams {
            name: "goal-evaluator",
            description: "judge",
            system_prompt: "sys",
            max_iterations: 1,
        })
    });
    assert_eq!(resolver("x").unwrap().name, "goal-evaluator");
}
