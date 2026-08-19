//! Subagent task and watchdog failure paths.

use std::sync::Arc;
use std::time::Duration;

use super::super::*;
use crate::multiagent::registry::AgentJobRegistry;
use crate::multiagent::types::AgentRunParams;
use theway_llm_provider::{
    AssistantMessageEvent, AssistantMessageEventStream, AssistantRole, ContentBlock, DoneReason,
    StopReason, Usage,
};

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

fn launch() -> AgentRunParams {
    AgentRunParams {
        name: "tester",
        description: "test",
        system_prompt: "sys",
        max_iterations: 1,
    }
}

fn done_stream(text: &str) -> AssistantMessageEventStream {
    let (stream, mut sender) = AssistantMessageEventStream::new();
    let msg = theway_llm_provider::AssistantMessage {
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
    stream
}

fn opts(stream_fn: StreamFn, timeout: Option<u64>) -> AgentRunOptions {
    AgentRunOptions {
        launch: launch(),
        tools: Vec::new(),
        prompt: "go".into(),
        model: faux_model(),
        stream_fn: Some(stream_fn),
        timeout,
        thinking: None,
        registry: AgentJobRegistry::new(),
        source: "dag".into(),
        run_id: None,
        node_id: None,
        session_id: None,
        observation_parent: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        system_prompt_extra: None,
        on_turn_end: None,
    }
}

#[tokio::test]
async fn run_agent_maps_join_error_to_subagent_task_failure() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| -> AssistantMessageEventStream {
        panic!("prompt task exploded")
    });

    let result = run_agent(opts(stream_fn, Some(1))).await;

    assert!(!result.success);
    assert!(
        result
            .error
            .as_deref()
            .unwrap()
            .contains("subagent task failed")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_agent_idle_timeout_force_kills_blocked_prompt() {
    let stream_fn: StreamFn = Arc::new(move |_, _, _| -> AssistantMessageEventStream {
        // Block the prompt task before the LLM loop can observe the abort token,
        // so the idle watchdog's grace period must elapse and force-kill.
        std::thread::sleep(Duration::from_secs(8));
        done_stream("late")
    });

    let result = run_agent(opts(stream_fn, Some(1))).await;

    assert!(!result.success);
    assert!(result.error.as_deref().unwrap().contains("Timed out"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_agent_handles_watchdog_overflow_panic() {
    // u64::MAX seconds overflows `std::time::Instant + Duration` inside the
    // watchdog task. The dropped oneshot sender races the prompt handle;
    // run_agent must always finish (via the prompt result or the timeout path).
    for _ in 0..200 {
        let stream_fn: StreamFn = Arc::new(move |_, _, _| done_stream("quick"));
        let _ = run_agent(opts(stream_fn, Some(u64::MAX))).await;
    }
}
