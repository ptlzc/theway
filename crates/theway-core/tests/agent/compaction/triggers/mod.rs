//! Tests for `agent::compaction::triggers` — split out of src
//! (see docs/rust-test-files.md).

use std::sync::Arc;

use crate::agent::assembly::{AgentHarness, AgentHarnessOptions};
use crate::agent::session::memory_storage::MemorySessionStorage;
use crate::agent::session::session::Session;
use theway_llm_provider::Model;

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

fn harness() -> AgentHarness {
    let session = Session::new(Arc::new(MemorySessionStorage::new()));
    AgentHarness::new(AgentHarnessOptions::new(faux_model(), session))
}

#[tokio::test]
async fn force_compact_returns_false_without_model() {
    let h = harness();
    h.agent().state().model = None;

    let compacted = h.force_compact(None).await.unwrap();

    assert!(!compacted);
}

#[tokio::test]
async fn force_compact_returns_false_for_empty_session() {
    let h = harness();

    let compacted = h.force_compact(Some("be concise".into())).await.unwrap();

    assert!(!compacted);
}

#[tokio::test]
async fn run_auto_compaction_skips_when_disabled() {
    let h = harness();
    h.compaction_settings.lock().enabled = false;

    assert!(h.run_auto_compaction().await.is_ok());
}

#[tokio::test]
async fn run_auto_compaction_skips_without_model() {
    let h = harness();
    h.agent().state().model = None;

    assert!(h.run_auto_compaction().await.is_ok());
}

#[tokio::test]
async fn run_auto_compaction_skips_below_threshold() {
    let h = harness();

    assert!(h.run_auto_compaction().await.is_ok());
    assert!(h.session().entries().await.unwrap().is_empty());
}
