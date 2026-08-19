//! Goal-mode evaluator node-ification (Phase ②): the `/goal` stop hook now runs the
//! evaluator as the goal run's node — a real agent run through the multiagent runner —
//! instead of a bare in-harness model call. Verifies:
//!   1. The evaluator registers as a DAG-source job under the goal run (run_id + "main"),
//!      with the node's job_id linked on the engine run.
//!   2. The evaluator job is controllable: GraphNodeInterrupt (registry.interrupt_node)
//!      aborts the evaluator turn and the hook pauses the goal.
//!   3. A normal "not done" evaluator reply drives the existing Continue flow.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use theway_core::multiagent::goal;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::RunKind;
use theway_core::multiagent::jobs::{SubagentJobRegistry, SubagentJobStatus};
use theway_core::multiagent::types::{AgentRunParams, AgentRunResolver};
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, AgentTool, MemorySessionStorage, Session,
    SessionStorage, StreamFn, TurnEndAction,
};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, Message as PiMessage, Model, ModelCost, StopReason, Usage,
};
use tokio_util::sync::CancellationToken;

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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

/// Scripted stream: `(text, delay_ms)` per call; empty text = stall forever.
fn sequence_stream(turns: Vec<(&'static str, u64)>) -> StreamFn {
    let calls = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_, _, _| {
        let n = calls.fetch_add(1, AtomicOrdering::SeqCst);
        let (text, delay) = turns[n.min(turns.len() - 1)];
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if text.is_empty() {
                return;
            }
            let msg = AssistantMessage {
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
            };
            sender.push(AssistantMessageEvent::Start {
                partial: msg.clone(),
            });
            sender.push(AssistantMessageEvent::Done {
                reason: DoneReason::Stop,
                message: msg,
            });
        });
        stream
    })
}

fn goal_evaluator_resolver() -> AgentRunResolver {
    let launch = AgentRunParams {
        name: "goal-evaluator",
        description: "test evaluator",
        system_prompt: goal::evaluator_system_prompt(),
        max_iterations: 1,
    };
    Arc::new(move |name: &str| (name == "goal-evaluator").then_some(launch))
}

fn build_harness(stream: StreamFn) -> (Arc<AgentHarness>, Arc<OnceLock<Arc<AgentHarness>>>) {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let cell: Arc<OnceLock<Arc<AgentHarness>>> = Arc::new(OnceLock::new());
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.stream_fn = Some(stream);
    let harness = Arc::new(AgentHarness::new(opts));
    let _ = cell.set(harness.clone());
    (harness, cell)
}

fn hook_ctx(transcript: Vec<AgentMessage>) -> theway_core::OnTurnEndContext {
    theway_core::OnTurnEndContext {
        transcript,
        continuation_count: 0,
        last_user_prompt: None,
    }
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(text.into()),
        timestamp: 0,
    }))
}

async fn wait_until<F: Fn() -> bool>(f: F) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met in time");
}

#[tokio::test]
async fn goal_evaluator_runs_as_node_job_and_continues() {
    // The hook is called directly (no main turn in this test): the first stream
    // call IS the evaluator ("not done" JSON) → Continue.
    let stream = sequence_stream(vec![
        (r#"{"ok": false, "reason": "missing evidence"}"#, 0),
        ("main reply 2", 0),
    ]);
    let (harness, cell) = build_harness(stream.clone());
    let engine = Arc::new(DagEngine::new());
    let registry = SubagentJobRegistry::new();
    let hook = goal::stop_hook(
        cell,
        engine.clone(),
        goal_evaluator_resolver(),
        registry.clone(),
        Some(stream),
    );
    goal::set(&harness, "answer the question".to_string())
        .await
        .unwrap();

    let decision = hook(
        hook_ctx(vec![user_msg("main turn transcript")]),
        CancellationToken::new(),
    )
    .await;

    assert!(
        matches!(decision.action, TurnEndAction::Continue { .. }),
        "not-done evaluator must Continue: {decision:?}"
    );

    // The evaluator ran as a real job: DAG source, goal run + "main" node.
    let jobs = registry.list();
    let evaluator = jobs
        .iter()
        .find(|j| j.source == "dag" && j.node_id.as_deref() == Some("main"))
        .expect("evaluator job registered");
    assert_eq!(evaluator.status, SubagentJobStatus::Succeeded);
    assert_eq!(evaluator.run_id.as_deref(), Some("goal-1"));
    // Transcript captured the evaluator's JSON reply.
    assert!(
        evaluator.messages.iter().any(|m| m["role"] == "assistant"
            && m["content"][0]["text"] == "{\"ok\": false, \"reason\": \"missing evidence\"}"),
        "evaluator reply in transcript: {:?}",
        evaluator.messages
    );

    // Engine node linked to the job id.
    let run = engine.get_run("goal-1").expect("goal run exists");
    assert_eq!(run.kind, RunKind::Goal);
    assert_eq!(
        run.node("main").unwrap().job_id.as_deref(),
        Some(evaluator.id.as_str())
    );
    // Goal state ticked once, still pursuing.
    let state = goal::current(&harness).await.unwrap();
    assert_eq!(state.iterations, 1);
    assert_eq!(state.status, goal::GoalStatus::Pursuing);
}

#[tokio::test]
async fn goal_evaluator_can_be_interrupted_via_node_control() {
    // The hook is called directly: the evaluator is the only LLM call, and it
    // stalls so we can interrupt it mid-turn.
    let stream = sequence_stream(vec![("", 30_000)]);
    let (harness, cell) = build_harness(stream.clone());
    let engine = Arc::new(DagEngine::new());
    let registry = SubagentJobRegistry::new();
    let hook = goal::stop_hook(
        cell,
        engine.clone(),
        goal_evaluator_resolver(),
        registry.clone(),
        Some(stream),
    );
    goal::set(&harness, "answer the question".to_string())
        .await
        .unwrap();

    let cancel = CancellationToken::new();
    let hook_fut = hook(hook_ctx(vec![user_msg("main turn")]), cancel.clone());
    let handle = tokio::spawn(hook_fut);

    // Wait for the evaluator job with its live control handle (the runner
    // registers the job first, the handle right after).
    wait_until(|| {
        registry.list().iter().any(|j| {
            j.source == "dag" && j.node_id.as_deref() == Some("main") && j.control.is_some()
        })
    })
    .await;

    // GraphNodeInterrupt path: interrupt the evaluator's in-flight turn.
    let jobs = registry.list();
    let evaluator = jobs
        .iter()
        .find(|j| j.source == "dag" && j.node_id.as_deref() == Some("main"))
        .unwrap();
    assert!(registry.interrupt(&evaluator.id), "interrupt accepted");

    let decision = handle.await.expect("hook task panicked");
    assert!(
        matches!(decision.action, TurnEndAction::Pause { .. }),
        "interrupted evaluator must pause the goal: {decision:?}"
    );
    let state = goal::current(&harness).await.unwrap();
    assert_eq!(state.status, goal::GoalStatus::Paused);
    assert!(
        state
            .last_reason
            .as_deref()
            .unwrap_or_default()
            .contains("interrupted"),
        "pause reason: {:?}",
        state.last_reason
    );
    // The engine run is cancelled so the UI shows a stopped goal.
    let run = engine.get_run("goal-1").expect("goal run exists");
    assert_eq!(
        run.status,
        theway_core::multiagent::graph::types::DagStatus::Cancelled
    );
}

// Silence unused-import warnings for types used only in helper signatures.
#[allow(dead_code)]
fn _assert_tool_send_sync(_: Box<dyn AgentTool>) {}
