//! Additional `turn/kernel` tests — split out of src, bridged from a nested
//! module so the primary `tests/turn/kernel/mod.rs` stays untouched.
//!
//! The primary suite covers construction/accessors and future builders; this
//! suite polls those futures against the deterministic faux provider so the
//! async blocks (prompt, multimodal prompt, template, compaction, continue)
//! actually execute.

use std::sync::Arc;

use theway_core::{
    AgentHarness, AgentHarnessOptions, AgentRunError, MemorySessionStorage, Session, SessionStorage,
};
use theway_llm_provider::{ImageContent, InputModality, ModelCost};
use tokio::time::Duration;

use super::super::*;
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

#[tokio::test]
async fn prompt_turn_polls_to_completion() {
    let (kernel, _harness) = kernel_with_input(Vec::new());

    let fut = kernel.prompt_turn("hello".into());
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("faux prompt should finish quickly")
        .expect("faux prompt should succeed");

    assert_eq!(result, None);
}

#[tokio::test]
async fn user_prompt_turn_text_only_polls_to_completion() {
    let (kernel, _harness) = kernel_with_input(Vec::new());

    let fut = kernel.user_prompt_turn("hello text-only".into(), Vec::new());
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("faux prompt should finish quickly")
        .expect("faux prompt should succeed");

    assert_eq!(result, None);
}

#[tokio::test]
async fn user_prompt_turn_with_images_polls_to_completion() {
    let (kernel, _harness) = kernel_with_input(vec![InputModality::Image]);

    let fut = kernel.user_prompt_turn(
        "hello with image".into(),
        vec![ImageContent {
            data: "aa".into(),
            mime_type: "image/png".into(),
        }],
    );
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("faux prompt should finish quickly")
        .expect("faux prompt should succeed");

    assert_eq!(result, None);
}

#[tokio::test]
async fn template_turn_reports_unknown_template_as_error() {
    let (kernel, _harness) = kernel_with_input(Vec::new());

    let fut = kernel.template_turn("missing-template".into(), serde_json::Map::new());
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("template turn should finish quickly")
        .expect_err("missing template should fail");

    match &result {
        AgentRunError::Other(msg) => assert!(
            msg.contains("unknown prompt template"),
            "unexpected error: {msg}"
        ),
        other => panic!("expected Other error, got {other:?}"),
    }
}

#[tokio::test]
async fn compaction_turn_reports_nothing_to_compact_on_empty_session() {
    let (kernel, _harness) = kernel_with_input(Vec::new());

    let fut = kernel.compaction_turn(None);
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("compaction should finish quickly")
        .expect("compaction should succeed on an empty session");

    assert_eq!(result.as_deref(), Some("nothing to compact"));
}

#[tokio::test]
async fn continue_turn_reports_error_on_empty_session() {
    let (kernel, _harness) = kernel_with_input(Vec::new());

    let fut = kernel.continue_turn();
    let result = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .expect("continue should finish quickly")
        .expect_err("continue on an empty session should fail");

    match &result {
        AgentRunError::Other(msg) => assert_eq!(msg, "No messages to continue from"),
        other => panic!("expected Other error, got {other:?}"),
    }
}

#[tokio::test]
#[should_panic(expected = "turn future present")]
async fn poll_turn_panics_when_future_is_none() {
    let mut fut: Option<TurnFut> = None;

    let _ = poll_turn(&mut fut).await;
}
