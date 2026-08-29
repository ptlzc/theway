use std::sync::Arc;

use parking_lot::Mutex;

use crate::observability::*;
use crate::{Agent, AgentMessage, AgentOptions, AgentState};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
    ContentBlock, DoneReason, Message, StopReason, ToolCall, Usage, UserContent, UserMessage,
    UserRole,
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<RuntimeObservation>>,
}

impl RuntimeObserver for RecordingObserver {
    fn observe(&self, observation: RuntimeObservation) {
        self.observations.lock().push(observation);
    }
}

#[test]
fn operation_scope_pairs_start_and_finish_with_parent() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let parent = OperationScope::start(
        observer.clone(),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    let child = OperationScope::start(
        observer,
        Some(parent.id()),
        ObservationContext::default().with_turn(0),
        OperationDetail::Turn { index: 0 },
    );
    child.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements {
            turns: 1,
            ..Default::default()
        },
    );
    parent.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements::default(),
    );

    let events = recording.observations.lock();
    assert_eq!(events.len(), 4);
    let RuntimeObservation::OperationStarted(parent_start) = &events[0] else {
        panic!("expected parent start");
    };
    let RuntimeObservation::OperationStarted(child_start) = &events[1] else {
        panic!("expected child start");
    };
    assert_eq!(child_start.parent_id, Some(parent_start.id));
    assert_eq!(child_start.context.turn_id, Some(0));
    let RuntimeObservation::OperationFinished(child_finish) = &events[2] else {
        panic!("expected child finish");
    };
    assert_eq!(child_finish.id, child_start.id);
    assert_eq!(child_finish.outcome, OperationOutcome::Succeeded);
    assert_eq!(child_finish.measurements.turns, 1);
}

#[test]
fn dropped_scope_finishes_as_abandoned_once() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let id = {
        let scope = OperationScope::start(
            observer,
            None,
            ObservationContext::default(),
            OperationDetail::AgentRun,
        );
        scope.id()
    };

    let events = recording.observations.lock();
    assert_eq!(events.len(), 2);
    let RuntimeObservation::OperationFinished(finish) = &events[1] else {
        panic!("expected finish");
    };
    assert_eq!(finish.id, id);
    assert_eq!(finish.outcome, OperationOutcome::Abandoned);
}

struct PanickingObserver;

impl RuntimeObserver for PanickingObserver {
    fn observe(&self, _observation: RuntimeObservation) {
        panic!("observer failure");
    }
}

#[test]
fn observer_panic_is_isolated() {
    let scope = OperationScope::start(
        Arc::new(PanickingObserver),
        None,
        ObservationContext::default(),
        OperationDetail::AgentRun,
    );
    scope.finish(
        OperationOutcome::Succeeded,
        None,
        RuntimeMeasurements::default(),
    );
}

#[test]
fn details_have_no_payload_or_raw_error_fields() {
    let detail = OperationDetail::ToolExecution {
        tool_name: "bash".into(),
    };
    let debug = format!("{detail:?}");
    assert_eq!(debug, "ToolExecution { tool_name: \"bash\" }");
    assert!(!debug.contains("args"));
    assert!(!debug.contains("result"));
    assert!(!debug.contains("message"));
}

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

fn done_stream() -> AssistantMessageEventStream {
    let (stream, mut sender) = AssistantMessageEventStream::new();
    let message = AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text("safe response")],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 11,
            output: 7,
            cache_read: 3,
            cache_write: 2,
            total_tokens: 23,
            prefix_hit_tokens: None,
            prefix_cache_hit_rate: None,
            provider_cache_hit_rate: None,
            cost: Default::default(),
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    };
    sender.push(AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message,
    });
    stream
}

#[tokio::test]
async fn agent_run_emits_correlated_run_turn_and_llm_operations() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        observer,
        observation_context: ObservationContext {
            session_id: Some("session-1".into()),
            ..Default::default()
        },
        stream_fn: Some(Arc::new(|_, _, _| done_stream())),
        ..Default::default()
    });
    let user = AgentMessage::Llm(Message::User(UserMessage {
        role: UserRole::User,
        content: UserContent::Text("SECRET_PROMPT".into()),
        timestamp: 0,
    }));

    agent.prompt(user).await.unwrap();

    let events = recording.observations.lock();
    let starts: Vec<&OperationStarted> = events
        .iter()
        .filter_map(|event| match event {
            RuntimeObservation::OperationStarted(start) => Some(start),
            RuntimeObservation::OperationFinished(_) => None,
        })
        .collect();
    assert_eq!(starts.len(), 3);
    assert!(matches!(starts[0].detail, OperationDetail::AgentRun));
    assert!(matches!(starts[1].detail, OperationDetail::Turn { index: 0 }));
    assert!(matches!(starts[2].detail, OperationDetail::LlmRequest { .. }));
    assert_eq!(starts[1].parent_id, Some(starts[0].id));
    assert_eq!(starts[2].parent_id, Some(starts[1].id));
    assert_eq!(starts[2].context.session_id.as_deref(), Some("session-1"));
    assert_eq!(starts[2].context.turn_id, Some(0));

    let llm_finish = events.iter().find_map(|event| match event {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::LlmRequest =>
        {
            Some(finish)
        }
        _ => None,
    });
    let llm_finish = llm_finish.expect("LLM finish observation");
    assert_eq!(llm_finish.outcome, OperationOutcome::Succeeded);
    assert_eq!(llm_finish.measurements.input_tokens, 11);
    assert_eq!(llm_finish.measurements.output_tokens, 7);
    assert!(!format!("{events:?}").contains("SECRET_PROMPT"));
}

#[tokio::test]
async fn failed_agent_observations_use_categories_without_raw_error() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let stream_fn: crate::StreamFn = Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let mut error = done_message("", Usage::default());
        error.stop_reason = StopReason::Error;
        error.error_message = Some("SECRET_PROVIDER_ERROR".into());
        sender.push(AssistantMessageEvent::Error {
            reason: theway_llm_provider::ErrorReason::Error,
            error,
        });
        stream
    });
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        observer,
        stream_fn: Some(stream_fn),
        ..Default::default()
    });

    let result = agent
        .prompt(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("go".into()),
            timestamp: 0,
        })))
        .await;
    assert!(result.is_err());

    let events = recording.observations.lock();
    let llm_finish = events.iter().find_map(|event| match event {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::LlmRequest =>
        {
            Some(finish)
        }
        _ => None,
    });
    let llm_finish = llm_finish.expect("LLM finish observation");
    assert_eq!(llm_finish.outcome, OperationOutcome::Failed);
    assert_eq!(llm_finish.error_category, Some(ErrorCategory::Provider));
    let run_finish = events.iter().find_map(|event| match event {
        RuntimeObservation::OperationFinished(finish)
            if finish.kind == OperationKind::AgentRun =>
        {
            Some(finish)
        }
        _ => None,
    });
    assert_eq!(
        run_finish.expect("agent run finish").outcome,
        OperationOutcome::Failed
    );
    assert!(!format!("{events:?}").contains("SECRET_PROVIDER_ERROR"));
}

#[tokio::test]
async fn aborting_agent_finishes_run_and_request_as_cancelled() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    let stream_fn: crate::StreamFn = Arc::new(move |_, _, _| {
        let (stream, sender) = AssistantMessageEventStream::new();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            drop(sender);
        });
        stream
    });
    let agent = Arc::new(Agent::new(AgentOptions {
        initial_state: Some(state),
        observer,
        stream_fn: Some(stream_fn),
        ..Default::default()
    }));
    let running = agent.clone();
    let prompt = tokio::spawn(async move {
        running
            .prompt(AgentMessage::Llm(Message::User(UserMessage {
                role: UserRole::User,
                content: UserContent::Text("go".into()),
                timestamp: 0,
            })))
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    agent.abort();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), prompt)
        .await
        .expect("aborted prompt completes")
        .expect("prompt task joins");

    let events = recording.observations.lock();
    for kind in [OperationKind::LlmRequest, OperationKind::AgentRun] {
        let finish = events.iter().find_map(|event| match event {
            RuntimeObservation::OperationFinished(finish) if finish.kind == kind => Some(finish),
            _ => None,
        });
        assert_eq!(
            finish.expect("cancelled finish").outcome,
            OperationOutcome::Cancelled
        );
    }
}

struct OkTool {
    definition: theway_llm_provider::Tool,
}

#[async_trait::async_trait]
impl crate::AgentTool for OkTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.definition
    }

    fn label(&self) -> &str {
        &self.definition.name
    }

    async fn execute(
        &self,
        _id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<crate::AgentToolUpdate>,
    ) -> Result<crate::AgentToolResult, crate::AgentToolError> {
        Ok(crate::AgentToolResult {
            content: vec![theway_llm_provider::UserContentBlock::text(
                "SECRET_TOOL_RESULT",
            )],
            details: serde_json::Value::Null,
            terminate: None,
        })
    }
}

fn queued_stream(
    responses: Arc<tokio::sync::Mutex<Vec<AssistantMessage>>>,
) -> crate::StreamFn {
    Arc::new(move |_, _, _| {
        let (stream, mut sender) = AssistantMessageEventStream::new();
        let responses = responses.clone();
        tokio::spawn(async move {
            let message = responses.lock().await.remove(0);
            let reason = if message.stop_reason == StopReason::ToolUse {
                DoneReason::ToolUse
            } else {
                DoneReason::Stop
            };
            sender.push(AssistantMessageEvent::Done { reason, message });
        });
        stream
    })
}

#[tokio::test]
async fn parallel_tools_are_siblings_and_payloads_are_absent() {
    let recording = Arc::new(RecordingObserver::default());
    let observer: Arc<dyn RuntimeObserver> = recording.clone();
    let arguments = serde_json::Map::from_iter([(
        "secret".into(),
        serde_json::Value::String("SECRET_TOOL_ARG".into()),
    )]);
    let first = AssistantMessage {
        content: vec![
            ContentBlock::ToolCall(ToolCall {
                id: "call-1".into(),
                name: "one".into(),
                arguments: arguments.clone(),
                thought_signature: None,
            }),
            ContentBlock::ToolCall(ToolCall {
                id: "call-2".into(),
                name: "two".into(),
                arguments,
                thought_signature: None,
            }),
        ],
        stop_reason: StopReason::ToolUse,
        ..done_message("ignored", Usage::default())
    };
    let second = done_message("complete", Usage::default());
    let responses = Arc::new(tokio::sync::Mutex::new(vec![first, second]));
    let mut state = AgentState::default();
    state.model = Some(faux_model());
    state.tools = vec![
        Arc::new(OkTool {
            definition: theway_llm_provider::Tool {
                name: "one".into(),
                description: "one".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }),
        Arc::new(OkTool {
            definition: theway_llm_provider::Tool {
                name: "two".into(),
                description: "two".into(),
                parameters: serde_json::json!({"type": "object"}),
            },
        }),
    ];
    let agent = Agent::new(AgentOptions {
        initial_state: Some(state),
        observer,
        stream_fn: Some(queued_stream(responses)),
        ..Default::default()
    });

    agent
        .prompt(AgentMessage::Llm(Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("run tools".into()),
            timestamp: 0,
        })))
        .await
        .unwrap();

    let events = recording.observations.lock();
    let tools: Vec<&OperationStarted> = events
        .iter()
        .filter_map(|event| match event {
            RuntimeObservation::OperationStarted(start)
                if matches!(start.detail, OperationDetail::ToolExecution { .. }) =>
            {
                Some(start)
            }
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].parent_id, tools[1].parent_id);
    assert!(tools[0].parent_id.is_some());
    let debug = format!("{events:?}");
    assert!(!debug.contains("SECRET_TOOL_ARG"));
    assert!(!debug.contains("SECRET_TOOL_RESULT"));
}

fn done_message(text: &str, usage: Usage) -> AssistantMessage {
    AssistantMessage {
        role: AssistantRole::Assistant,
        content: vec![ContentBlock::text(text)],
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        model: "faux".into(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage,
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}
