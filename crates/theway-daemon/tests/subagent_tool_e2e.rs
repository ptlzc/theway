//! End-to-end test for the subagent tool (issue #11).
//!
//! Drives `SubagentTool::execute` with a faux StreamFn shared with the inner subagent harness.
//! Verifies:
//!   1. The tool returns the subagent's final assistant text.
//!   2. Unknown subagent_type errors clearly.
//!   3. Missing required `prompt` arg errors clearly.

use std::sync::Arc;

use theway_core::{AgentTool, StreamFn};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, ModelCost, StopReason, Usage,
};
use tokio_util::sync::CancellationToken;

use theway_daemon::tools::subagent;

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
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn test_launch_resolver() -> theway_core::multiagent::types::AgentRunResolver {
    let launch = theway_core::multiagent::types::AgentRunParams {
        name: "general",
        description: "test",
        system_prompt: "You are a test subagent.",
        max_iterations: 16,
    };
    std::sync::Arc::new(move |name: &str| (name == "general").then_some(launch))
}

fn faux_stream(text: &'static str) -> StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
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

#[tokio::test]
async fn subagent_returns_final_text() {
    let tool = subagent::SubagentTool::new(
        faux_model(),
        Some(faux_stream("subagent result")),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
        vec!["general".to_string()],
        theway_core::multiagent::jobs::SubagentJobRegistry::new(),
    );
    let res = tool
        .execute(
            "t-1",
            serde_json::json!({
                "subagent_type": "general",
                "description": "look up X",
                "prompt": "tell me about X",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    let body = match &res.content[0] {
        theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
        _ => panic!("expected text content"),
    };
    assert_eq!(body, "subagent result");
}

#[tokio::test]
async fn subagent_without_model_errors_clearly() {
    // Model is session-level (injected by the client): a subagent tool built with
    // no model must fail with a clear error instead of spawning a model-less run.
    let tool = subagent::SubagentTool::new(
        None,
        Some(faux_stream("nope")),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
        vec!["general".to_string()],
        theway_core::multiagent::jobs::SubagentJobRegistry::new(),
    );
    let err = tool
        .execute(
            "t-no-model",
            serde_json::json!({
                "subagent_type": "general",
                "description": "look up X",
                "prompt": "tell me about X",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no model set for this session"), "{err}");
}

#[tokio::test]
async fn subagent_unknown_type_errors() {
    let tool = subagent::SubagentTool::new(
        faux_model(),
        Some(faux_stream("nope")),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
        vec!["general".to_string()],
        theway_core::multiagent::jobs::SubagentJobRegistry::new(),
    );
    let err = tool
        .execute(
            "t-2",
            serde_json::json!({
                "subagent_type": "nope",
                "prompt": "x",
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown subagent_type"), "{err}");
}

#[tokio::test]
async fn subagent_missing_prompt_errors() {
    let tool = subagent::SubagentTool::new(
        faux_model(),
        Some(faux_stream("nope")),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
        vec!["general".to_string()],
        theway_core::multiagent::jobs::SubagentJobRegistry::new(),
    );
    let err = tool
        .execute("t-3", serde_json::json!({}), CancellationToken::new(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing required arg: prompt"), "{err}");
}

#[tokio::test]
async fn subagent_parent_abort_cascades() {
    // Stalled subagent stream: subagent never finishes on its own; only parent abort can
    // unblock it.
    let stalled: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let _sender = sender;
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        stream
    });
    let tool = subagent::SubagentTool::new(
        faux_model(),
        Some(stalled),
        Arc::new(|_| Vec::new()),
        test_launch_resolver(),
        vec!["general".to_string()],
        theway_core::multiagent::jobs::SubagentJobRegistry::new(),
    );
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    let exec = tokio::spawn(async move {
        tool.execute("t-4", serde_json::json!({ "prompt": "x" }), cancel2, None)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), exec)
        .await
        .expect("parent abort must unblock subagent within 2s")
        .expect("task panicked");
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("cancel") || err.to_lowercase().contains("abort"),
        "expected abort error: {err}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Turn-level control: interrupt (stop the current turn) + steer (inject at the
// next natural turn boundary), exercised through `run_agent` + registry.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;

use theway_core::multiagent::jobs::{SubagentJobRegistry, SubagentJobStatus};
use theway_core::multiagent::runner::{AgentRunOptions, run_agent};

/// Per-call scripted stream: `(text, delay_ms)` for each LLM call; calls past the
/// last entry repeat it. Empty text = stall `delay_ms` then end without emitting
/// (an interrupted turn's view of a dead stream).
fn sequence_stream(turns: Vec<(&'static str, u64)>) -> StreamFn {
    let calls = StdArc::new(AtomicUsize::new(0));
    StdArc::new(move |_, _, _| {
        let n = calls.fetch_add(1, AtomicOrdering::SeqCst);
        let (text, delay) = turns[n.min(turns.len() - 1)];
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if text.is_empty() {
                // Stalled / dead stream: no events at all.
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

/// First call stalls forever (dead stream), later calls reply `text`.
fn stall_then_reply(text: &'static str) -> StreamFn {
    sequence_stream(vec![("", 30_000), (text, 0)])
}

async fn wait_for_job(registry: &SubagentJobRegistry) -> String {
    for _ in 0..100 {
        if let Some(job) = registry.list().first() {
            return job.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job never registered");
}

fn run_options(
    registry: SubagentJobRegistry,
    stream: StreamFn,
    cancel: CancellationToken,
) -> AgentRunOptions {
    AgentRunOptions {
        launch: test_launch_resolver()("general").unwrap(),
        tools: Vec::new(),
        prompt: "explore X".into(),
        model: faux_model(),
        stream_fn: Some(stream),
        timeout: None,
        thinking: None,
        registry,
        source: "subagent".into(),
        run_id: None,
        node_id: None,
        session_id: None,
        observation_parent: None,
        cancel,
        system_prompt_extra: None,
        on_turn_end: None,
    }
}

#[tokio::test]
async fn interrupt_without_steer_ends_run_interrupted() {
    let registry = SubagentJobRegistry::new();
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(run_agent(run_options(
        registry.clone(),
        stall_then_reply("done"),
        cancel,
    )));

    let id = wait_for_job(&registry).await;
    // Let the first turn reach the stalled LLM call.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(registry.interrupt(&id), "interrupt accepted");
    let result = handle.await.expect("run task panicked");
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("turn interrupted"));

    let job = registry.job(&id).unwrap();
    assert_eq!(job.status, SubagentJobStatus::Interrupted);
    assert!(job.control.is_none(), "control detached after finish");
}

#[tokio::test]
async fn interrupt_with_steer_continues_into_next_turn() {
    let registry = SubagentJobRegistry::new();
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(run_agent(run_options(
        registry.clone(),
        stall_then_reply("done after steer"),
        cancel,
    )));

    let id = wait_for_job(&registry).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Queue the steer BEFORE interrupting so the drained queue is never empty.
    assert!(registry.steer(&id, "abandon plan A, use plan B".into()));
    assert!(registry.interrupt(&id), "interrupt accepted");

    let result = handle.await.expect("run task panicked");
    assert!(
        result.success,
        "run continues after steer: {:?}",
        result.error
    );
    assert_eq!(result.text, "done after steer");

    let job = registry.job(&id).unwrap();
    assert_eq!(job.status, SubagentJobStatus::Succeeded);
    // The steer message landed in the transcript at the next turn.
    let steered = job
        .messages
        .iter()
        .any(|m| m["role"] == "user" && m["content"] == "abandon plan A, use plan B");
    assert!(
        steered,
        "steer message present in transcript: {:?}",
        job.messages
    );
}

#[tokio::test]
async fn steer_mid_turn_lands_at_next_natural_turn() {
    // Steer without interrupt: queued while the first turn is in flight (300 ms
    // scripted delay), drained at the natural turn boundary, next turn carries it.
    let registry = SubagentJobRegistry::new();
    let cancel = CancellationToken::new();
    let handle = tokio::spawn(run_agent(run_options(
        registry.clone(),
        sequence_stream(vec![("first turn", 300), ("second turn", 0)]),
        cancel,
    )));

    let id = wait_for_job(&registry).await;
    // Queue the steer while the first turn is still streaming (delay 300ms).
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(registry.steer(&id, "nudge".into()));

    let result = handle.await.expect("run task panicked");
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.text, "second turn", "steer routes the next turn");
    let job = registry.job(&id).unwrap();
    let steered = job
        .messages
        .iter()
        .any(|m| m["role"] == "user" && m["content"] == "nudge");
    assert!(steered, "steer at next natural turn: {:?}", job.messages);
}
