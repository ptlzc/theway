//! `/goal` command suite: set/report/clear, `/goal start` (+ shortcut), and the
//! goal evaluator stop-hook continuation path.

use std::sync::Arc;
use std::sync::OnceLock;

use theway_core::multiagent::goal;
use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentMessage, MemorySessionStorage, OnTurnEndContext,
    Session, SessionStorage, SessionTreeEntry, TurnEndAction,
};
use theway_llm_provider::Message;

use super::helpers::*;
use crate::commands;

#[tokio::test]
async fn dispatch_goal_sets_and_reports_session_goal() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome =
        commands::dispatch("/goal finish only after cargo test passes", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let state = goal::current(&harness).await.expect("goal state");
    assert_eq!(state.status, goal::GoalStatus::Pursuing);
    assert_eq!(state.condition, "finish only after cargo test passes");

    let outcome = commands::dispatch("/goal", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    let output = capture.text();
    assert!(output.contains("goal set: finish only after cargo test passes"));
    assert!(
        output.contains("start by sending a normal prompt, or run /goal-start <prompt>"),
        "{output}"
    );
    assert!(output.contains("status: pursuing"), "{output}");
    assert!(output.contains("iterations: 0"), "{output}");

    let entries = session.entries().await.unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            SessionTreeEntry::Custom { custom_type, data, .. }
                if custom_type == goal::CUSTOM_TYPE
                    && data.as_ref().is_some_and(|d| d["condition"] == "finish only after cargo test passes")
        )),
        "goal command must persist session metadata: {entries:#?}"
    );
}

#[tokio::test]
async fn dispatch_goal_start_runs_prompt_when_goal_active() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    goal::set(&harness, "finish only after cargo test passes".into())
        .await
        .unwrap();

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome = commands::dispatch("/goal start run cargo test", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunAgentPrompt {
            prompt,
            error_context,
        } => {
            assert_eq!(prompt, "run cargo test");
            assert_eq!(error_context, "goal start: ");
        }
        other => panic!("expected RunAgentPrompt, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_goal_start_shortcut_runs_prompt_when_goal_active() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    goal::set(&harness, "finish only after cargo test passes".into())
        .await
        .unwrap();

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome = commands::dispatch("/goal-start run cargo test", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::RunAgentPrompt {
            prompt,
            error_context,
        } => {
            assert_eq!(prompt, "run cargo test");
            assert_eq!(error_context, "goal start: ");
        }
        other => panic!("expected RunAgentPrompt, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_goal_start_requires_active_goal() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let _capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session.clone());
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome = commands::dispatch("/goal start run cargo test", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(message) => {
            assert!(message.contains("no active goal"), "{message}");
            assert!(message.contains("/goal <condition>"), "{message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let outcome = commands::dispatch("/goal-start run cargo test", &registry, &ctx).await;
    match outcome {
        commands::CommandOutcome::Error(message) => {
            assert!(message.contains("no active goal"), "{message}");
            assert!(message.contains("/goal <condition>"), "{message}");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_goal_clear_hides_current_goal() {
    let _guard = COMMAND_OUTPUT_LOCK.lock().unwrap();
    let capture = OutputCapture::install();
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let opts = AgentHarnessOptions::new(faux_model(), session);
    let harness = Arc::new(AgentHarness::new(opts));
    let executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    goal::set(&harness, "ship a release".into()).await.unwrap();

    let registry = commands::Registry::with_builtins();
    let cwd = std::env::current_dir().unwrap();
    let ctx = commands::CommandCtx {
        harness: &harness,
        trigger_executor: &executor,
        session_id: "test",
        log_path: None,
        tool_count: 0,
        cwd: &cwd,
    };

    let outcome = commands::dispatch("/goal clear", &registry, &ctx).await;
    assert!(matches!(outcome, commands::CommandOutcome::Handled));

    assert!(goal::current(&harness).await.is_none());
    let output = capture.text();
    assert!(output.contains("goal cleared"), "{output}");
}

#[tokio::test]
async fn goal_evaluator_false_returns_continuation_and_audits_reason() {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session.clone());
    opts.stream_fn = Some(Arc::new(|_, _, _| {
        stream_one(assistant_text(
            "{\"ok\":false,\"reason\":\"missing cargo test output\"}",
        ))
    }));
    let harness = Arc::new(AgentHarness::new(opts));
    let _executor = Arc::new(crate::trigger_engine::execution::TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let harness_cell = Arc::new(OnceLock::new());
    assert!(harness_cell.set(harness.clone()).is_ok());
    let registry = theway_core::multiagent::jobs::SubagentJobRegistry::new();
    let hook = goal::stop_hook(
        harness_cell,
        std::sync::Arc::new(theway_core::multiagent::graph::engine::DagEngine::new()),
        std::sync::Arc::new(|name: &str| {
            (name == "goal-evaluator").then_some(theway_core::multiagent::types::AgentRunParams {
                name: "goal-evaluator",
                description: "test",
                system_prompt: goal::evaluator_system_prompt(),
                max_iterations: 1,
            })
        }),
        registry,
        Some(Arc::new(|_, _, _| {
            stream_one(assistant_text(
                "{\"ok\":false,\"reason\":\"missing cargo test output\"}",
            ))
        })),
    );
    goal::set(&harness, "finish only after cargo test passes".into())
        .await
        .unwrap();

    let decision = hook(
        OnTurnEndContext {
            transcript: vec![AgentMessage::Llm(Message::User(
                theway_llm_provider::UserMessage {
                    role: theway_llm_provider::UserRole::User,
                    content: theway_llm_provider::UserContent::Text("ran cargo build only".into()),
                    timestamp: 0,
                },
            ))],
            continuation_count: 0,
            last_user_prompt: Some("ran cargo build only".into()),
        },
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
    let TurnEndAction::Continue { prompt } = decision.action else {
        panic!("expected continuation, got {:?}", decision.action);
    };
    assert!(prompt.contains("finish only after cargo test passes"));
    assert!(prompt.contains("missing cargo test output"));
    assert_eq!(decision.payload.as_ref().unwrap()["ok"], false);
    assert_eq!(
        decision.payload.as_ref().unwrap()["reason"],
        "missing cargo test output"
    );

    let state = goal::current(&harness).await.expect("goal state");
    assert_eq!(state.iterations, 1);
    assert_eq!(
        state.last_reason.as_deref(),
        Some("missing cargo test output")
    );

    let entries = session.entries().await.unwrap();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            SessionTreeEntry::Custom { custom_type, data, .. }
                if custom_type == goal::CUSTOM_TYPE
                    && data.as_ref().is_some_and(|d| d["status"] == "pursuing"
                        && d["last_reason"] == "missing cargo test output")
        )),
        "goal hook must persist updated goal state: {entries:#?}"
    );
}
