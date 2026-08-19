//! Tests for `turn/kernel` — split out of src (see docs/rust-test-files.md).

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentRunError, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{ImageContent, InputModality, ModelCost};

use super::*;
use crate::agent_session::RetrySettings;
use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::runtime::TriggerRuntimeConfig;

fn faux_model(input: Vec<InputModality>) -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input,
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

fn harness_with_input(input: Vec<InputModality>) -> Arc<AgentHarness> {
    let storage = Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as Arc<dyn SessionStorage>);
    Arc::new(AgentHarness::new(AgentHarnessOptions::new(
        faux_model(input),
        session,
    )))
}

fn kernel_with_input(input: Vec<InputModality>) -> (ReplKernel, Arc<AgentHarness>) {
    let harness = harness_with_input(input);
    let trigger_executor = Arc::new(TriggerExecutor::new(
        harness.agent_arc(),
        harness.session().clone(),
        TriggerRuntimeConfig::default(),
        None,
        None,
        None,
        None,
        None,
        None,
    ));
    let kernel = ReplKernel::new(harness.clone(), trigger_executor, RetrySettings::default());
    (kernel, harness)
}

#[test]
fn turn_state_defaults_to_no_future() {
    let state = TurnState::default();

    assert!(state.fut.is_none());
    assert!(!state.aborted);
    assert_eq!(state.prefix, "");
}

#[tokio::test]
async fn poll_turn_polls_ready_future() {
    let mut fut: Option<TurnFut> = Some(Box::pin(async {
        Ok::<Option<String>, AgentRunError>(Some("done".to_string()))
    }));

    let result = poll_turn(&mut fut).await;

    assert_eq!(result.unwrap(), Some("done".to_string()));
    assert!(fut.is_some(), "future stays installed after completion");
}

#[tokio::test]
async fn poll_turn_propagates_run_error() {
    let mut fut: Option<TurnFut> = Some(Box::pin(async {
        Err::<Option<String>, AgentRunError>(AgentRunError::Other("boom".into()))
    }));

    let result = poll_turn(&mut fut).await;

    match result {
        Err(AgentRunError::Other(msg)) => assert_eq!(msg, "boom"),
        other => panic!("expected Other error, got {other:?}"),
    }
}

#[test]
fn queued_turn_display_reports_each_variant() {
    assert_eq!(
        QueuedTurn::UserPrompt {
            display: "user".into(),
            prompt: "p".into(),
            images: Vec::<ImageContent>::new(),
        }
        .display(),
        "user"
    );
    assert_eq!(
        QueuedTurn::AgentPrompt {
            display: "agent".into(),
            prompt: "p".into(),
            error_context: "ctx",
        }
        .display(),
        "agent"
    );
    assert_eq!(
        QueuedTurn::PromptTemplate {
            display: "template".into(),
            name: "n".into(),
            vars: serde_json::Map::new(),
        }
        .display(),
        "template"
    );
    assert_eq!(
        QueuedTurn::Compaction {
            display: "compact".into(),
            custom: None,
        }
        .display(),
        "compact"
    );
}

#[test]
fn kernel_exposes_harness_trigger_executor_and_model_capability() {
    let (kernel, harness) = kernel_with_input(Vec::new());

    assert!(Arc::ptr_eq(kernel.harness(), &harness));
    // Trigger executor is installed and can be reached through the accessor.
    kernel.trigger_executor().abort();
    assert!(!kernel.is_streaming());
    // Faux model has no image input modality.
    assert!(!kernel.current_model_accepts_images());
}

#[test]
fn kernel_current_model_accepts_images_detects_vision_input() {
    let (kernel, _harness) = kernel_with_input(vec![InputModality::Image]);

    assert!(kernel.current_model_accepts_images());
}

#[test]
fn kernel_replace_runtime_swaps_all_session_scoped_services() {
    let (mut kernel, original) = kernel_with_input(Vec::new());
    let replacement = harness_with_input(vec![InputModality::Image]);
    let runtime =
        crate::orchestration::SessionRuntime::for_test("replacement", replacement.clone());
    let replacement_trigger_executor = runtime.trigger_executor.clone();

    kernel.replace_runtime(runtime);

    assert!(Arc::ptr_eq(kernel.harness(), &replacement));
    assert!(!Arc::ptr_eq(kernel.harness(), &original));
    assert!(Arc::ptr_eq(
        kernel.trigger_executor(),
        &replacement_trigger_executor
    ));
    // Retry settings and model capability follow the replacement harness.
    assert!(kernel.current_model_accepts_images());
}

#[test]
fn kernel_turn_builders_return_futures_without_starting_a_run() {
    let (kernel, harness) = kernel_with_input(Vec::new());
    let mut vars = serde_json::Map::new();
    vars.insert("k".to_string(), serde_json::json!("v"));

    drop(kernel.prompt_turn("plain".into()));
    drop(kernel.user_prompt_turn("text-only".into(), Vec::new()));
    drop(kernel.user_prompt_turn(
        "with-image".into(),
        vec![ImageContent {
            data: "aa".into(),
            mime_type: "image/png".into(),
        }],
    ));
    drop(kernel.template_turn("tpl".into(), vars));
    drop(kernel.compaction_turn(Some("custom".into())));
    drop(kernel.continue_turn());

    // Building a turn future must not mark the harness streaming; the caller
    // polls it later.
    assert!(!harness.agent().is_streaming());
    kernel.abort();
}
