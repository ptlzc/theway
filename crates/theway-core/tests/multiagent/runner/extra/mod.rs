//! Extra tests for `multiagent::runner` — bridged through `runner_extra_tests`.

use std::sync::Arc;

use super::super::*;
use crate::types::StreamFn;
use crate::multiagent::registry::{AgentJobRegistry, JobStatus};
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

#[tokio::test]
async fn run_agent_succeeds_and_registers_job() {
    let registry = AgentJobRegistry::new();
    let stream_fn: StreamFn = Arc::new(move |_, _, _| done_stream("hello subagent"));
    let on_turn_end: Arc<dyn Fn(&str, u64, u64) + Send + Sync> =
        Arc::new(|_text: &str, _input: u64, _output: u64| {});
    let result = run_agent(AgentRunOptions {
        launch: AgentRunParams {
            name: "tester",
            description: "test",
            system_prompt: "sys",
            max_iterations: 1,
        },
        tools: Vec::new(),
        prompt: "go".into(),
        model: faux_model(),
        stream_fn: Some(stream_fn),
        timeout: Some(0),
        thinking: Some("off".into()),
        registry: registry.clone(),
        source: "dag".into(),
        run_id: Some("dag-1".into()),
        node_id: Some("a".into()),
        session_id: Some("sess-1".into()),
        cancel: tokio_util::sync::CancellationToken::new(),
        system_prompt_extra: Some("extra".into()),
        on_turn_end: Some(on_turn_end),
    })
    .await;

    assert!(result.success);
    assert_eq!(result.text, "hello subagent");
    assert!(result.error.is_none());

    let job = registry.job(&result.job_id).unwrap();
    assert_eq!(job.status, JobStatus::Succeeded);
    assert_eq!(job.source, "dag");
    assert_eq!(job.run_id.as_deref(), Some("dag-1"));
    assert_eq!(job.node_id.as_deref(), Some("a"));
    assert_eq!(job.session_id.as_deref(), Some("sess-1"));
}

#[tokio::test]
async fn run_agent_returns_cancelled_when_parent_cancel_fires() {
    let registry = AgentJobRegistry::new();
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        run_agent(AgentRunOptions {
            launch: AgentRunParams {
                name: "tester",
                description: "test",
                system_prompt: "sys",
                max_iterations: 1,
            },
            tools: Vec::new(),
            prompt: "go".into(),
            model: faux_model(),
            stream_fn: Some(stream_fn),
            timeout: Some(0),
            thinking: None,
            registry,
            source: "dag".into(),
            run_id: None,
            node_id: None,
            session_id: None,
            cancel: cancel_clone,
            system_prompt_extra: None,
            on_turn_end: None,
        })
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    cancel.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("run_agent must finish after cancel")
        .expect("task join");

    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn run_agent_idle_timeout_reports_timeout_error() {
    let registry = AgentJobRegistry::new();
    // A stream that never produces an event keeps the LLM call pending until
    // the idle watchdog aborts it.
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });

    let start = std::time::Instant::now();
    let result = run_agent(AgentRunOptions {
        launch: AgentRunParams {
            name: "tester",
            description: "test",
            system_prompt: "sys",
            max_iterations: 1,
        },
        tools: Vec::new(),
        prompt: "go".into(),
        model: faux_model(),
        stream_fn: Some(stream_fn),
        timeout: Some(1),
        thinking: None,
        registry,
        source: "dag".into(),
        run_id: None,
        node_id: None,
        session_id: None,
        cancel: tokio_util::sync::CancellationToken::new(),
        system_prompt_extra: None,
        on_turn_end: None,
    })
    .await;

    let elapsed = start.elapsed();
    assert!(!result.success);
    assert!(result.error.as_deref().unwrap().contains("Timed out"));
    assert!(
        elapsed < std::time::Duration::from_secs(8),
        "idle timeout should finish well under the force-kill grace, took {elapsed:?}"
    );
}

#[tokio::test]
async fn run_agent_interrupted_by_control_handle_marks_job_interrupted() {
    let registry = AgentJobRegistry::new();
    // A stream that never produces an event keeps the LLM call pending until
    // the control handle interrupts the in-flight turn.
    let stream_fn: StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });
    let run_registry = registry.clone();
    let handle = tokio::spawn(async move {
        run_agent(AgentRunOptions {
            launch: AgentRunParams {
                name: "tester",
                description: "test",
                system_prompt: "sys",
                max_iterations: 1,
            },
            tools: Vec::new(),
            prompt: "go".into(),
            model: faux_model(),
            stream_fn: Some(stream_fn),
            timeout: Some(0),
            thinking: None,
            registry: run_registry.clone(),
            source: "dag".into(),
            run_id: None,
            node_id: None,
            session_id: None,
            cancel: tokio_util::sync::CancellationToken::new(),
            system_prompt_extra: None,
            on_turn_end: None,
        })
        .await
    });

    // Wait for the registry job to appear, then interrupt it.
    let job_id = loop {
        let jobs = registry.list();
        if !jobs.is_empty() {
            break jobs[0].id.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert!(registry.interrupt(&job_id));

    let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
        .await
        .expect("run_agent must finish after interrupt")
        .expect("task join");

    assert!(!result.success);
    assert!(result.error.is_some());
    let job = registry.job(&job_id).unwrap();
    assert_eq!(job.status, JobStatus::Interrupted);
}
