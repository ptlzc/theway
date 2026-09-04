//! Node launcher cancellation and tracing behavior.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use super::super::*;
use crate::multiagent::graph::types::{DagNodeDef, DagRunDef};
use crate::multiagent::jobs::SubagentJobRegistry;
use crate::multiagent::types::{AgentRunParams, AgentRunResolver, ToolSetResolver};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, StopReason,
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
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        headers: None,
        compat: None,
    }
}

fn quick_stream() -> StreamFn {
    Arc::new(|_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            let msg = AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::text("ok")],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "faux".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: theway_llm_provider::Usage::default(),
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

fn launch_resolver() -> AgentRunResolver {
    Arc::new(|name: &str| {
        (name == "general").then_some(AgentRunParams {
            name: "general",
            description: "test agent",
            system_prompt: "sys",
            max_iterations: 1,
        })
    })
}

fn tools_resolver() -> ToolSetResolver {
    Arc::new(|_| Vec::new())
}

fn engine_with_run() -> (DagEngine, String) {
    let engine = DagEngine::new();
    let run = engine
        .plan(
            DagRunDef {
                name: "linecov".into(),
                nodes: vec![DagNodeDef {
                    id: "a".into(),
                    agent: "general".into(),
                    task: "task".into(),
                    depends_on: None,
                    timeout: None,
                    cwd: None,
                    provider: None,
                    model: None,
                    thinking: None,
                    max_iterations: None,
                    tools: None,
                }],
                max_concurrency: None,
                fail_fast: None,
                direction: None,
            },
            None,
            None,
        )
        .unwrap();
    let id = run.id.clone();
    (engine, id)
}

#[test]
fn launch_returns_early_when_cancel_is_already_cancelled() {
    let (engine, run_id) = engine_with_run();
    let launcher = node_launcher(
        Arc::new(engine),
        Some(faux_model()),
        None,
        PathBuf::from("."),
        SubagentJobRegistry::new(),
        tools_resolver(),
        launch_resolver(),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();

    NodeLauncher::launch(launcher.as_ref(), &run_id, "a", cancel);
}

struct DebugSubscriber;

impl tracing::subscriber::Subscriber for DebugSubscriber {
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::always()
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::DEBUG)
    }

    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.level() <= &tracing::Level::DEBUG
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn event(&self, _event: &tracing::Event<'_>) {}

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(
        &self,
        _span: &tracing::span::Id,
        _follows: &tracing::span::Id,
    ) {
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::test]
async fn launch_logs_debug_fields_when_subscriber_enabled() {
    let (engine, run_id) = engine_with_run();
    let launcher = node_launcher(
        Arc::new(engine),
        Some(faux_model()),
        Some(quick_stream()),
        PathBuf::from("."),
        SubagentJobRegistry::new(),
        tools_resolver(),
        launch_resolver(),
    );

    tracing::subscriber::with_default(DebugSubscriber, || {
        NodeLauncher::launch(launcher.as_ref(), &run_id, "a", CancellationToken::new());
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
}
