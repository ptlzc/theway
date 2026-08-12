//! Shared REPL execution kernel for terminal and future web frontends.
//!
//! This module owns the "what work should the agent run" boundary: prompt futures, abort, model
//! capability checks, and queued-turn value types. The terminal UI still owns rendering and
//! keyboard/mouse handling, but it should not construct harness futures directly. Keeping that
//! split narrow lets the upcoming web UI reuse the same turn semantics without copying TUI code.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::agent_session::{AgentSession, RetrySettings};
use theway_core::{AgentHarness, AgentRunError};
use theway_llm_provider::{ImageContent, InputModality};

/// In-flight model turn, polled by a frontend event loop.
///
/// Running this as a local future (not `tokio::spawn`) sidesteps the `Send` bound:
/// `AgentSession::prompt` briefly holds a `parking_lot` guard across an `.await`, so its future is
/// `!Send`.
pub(super) type TurnFut = Pin<Box<dyn Future<Output = Result<Option<String>, AgentRunError>>>>;

#[derive(Default)]
pub(super) struct TurnState {
    pub(super) fut: Option<TurnFut>,
    pub(super) aborted: bool,
    /// Prefix for the error line if the turn fails (e.g. `triggered turn: `).
    pub(super) prefix: &'static str,
}

pub(super) async fn poll_turn(fut: &mut Option<TurnFut>) -> Result<Option<String>, AgentRunError> {
    // Only created by `select!` when `fut.is_some()`, so the unwrap is sound.
    fut.as_mut().expect("turn future present").await
}

pub(super) enum QueuedTurn {
    UserPrompt {
        display: String,
        prompt: String,
        images: Vec<ImageContent>,
    },
    AgentPrompt {
        display: String,
        prompt: String,
        error_context: &'static str,
    },
    PromptTemplate {
        display: String,
        name: String,
        vars: serde_json::Map<String, serde_json::Value>,
    },
    Compaction {
        display: String,
        custom: Option<String>,
    },
}

impl QueuedTurn {
    pub(super) fn display(&self) -> &str {
        match self {
            Self::UserPrompt { display, .. }
            | Self::AgentPrompt { display, .. }
            | Self::PromptTemplate { display, .. }
            | Self::Compaction { display, .. } => display,
        }
    }
}

#[derive(Clone)]
pub(super) struct ReplKernel {
    harness: Arc<AgentHarness>,
    trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
    retry: RetrySettings,
}

impl ReplKernel {
    pub(super) fn new(
        harness: Arc<AgentHarness>,
        trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
        retry: RetrySettings,
    ) -> Self {
        Self {
            harness,
            trigger_executor,
            retry,
        }
    }

    pub(super) fn trigger_executor(
        &self,
    ) -> &Arc<crate::trigger_engine::execution::TriggerExecutor> {
        &self.trigger_executor
    }

    pub(super) fn harness(&self) -> &Arc<AgentHarness> {
        &self.harness
    }

    /// Swap in a different harness (session-resource-model: in-process session switch).
    ///
    /// Only the harness field changes — `retry` settings are session-independent and stay.
    /// Callers must guarantee no turn is in flight on the old harness (or have requested its
    /// abort): the event loop is serialized, so the transport loop / TUI loop calling this
    /// between turns is safe. An in-flight turn future holds its own `Arc` clone of the old
    /// harness and unwinds independently.
    pub(super) fn replace_harness(&mut self, harness: Arc<AgentHarness>) {
        self.harness = harness;
    }

    pub(super) fn abort(&self) {
        self.harness.abort();
    }

    pub(super) fn is_streaming(&self) -> bool {
        self.harness.agent().is_streaming()
    }

    pub(super) fn current_model_accepts_images(&self) -> bool {
        let state = self.harness.agent().state();
        state
            .model
            .as_ref()
            .map(|model| model.input.contains(&InputModality::Image))
            .unwrap_or(false)
    }

    pub(super) fn prompt_turn(&self, prompt: String) -> TurnFut {
        let harness = self.harness.clone();
        Box::pin(async move { harness.prompt(prompt).await.map(|_| None) })
    }

    pub(super) fn user_prompt_turn(
        &self,
        prompt_text: String,
        loaded_images: Vec<ImageContent>,
    ) -> TurnFut {
        let harness = self.harness.clone();
        let retry = self.retry.clone();
        let has_images = !loaded_images.is_empty();
        Box::pin(async move {
            if has_images {
                harness
                    .prompt_with_images(prompt_text, loaded_images)
                    .await
                    .map(|_| None)
            } else {
                AgentSession::new(harness, retry)
                    .prompt(prompt_text)
                    .await
                    .map(|_| None)
            }
        })
    }

    pub(super) fn template_turn(
        &self,
        name: String,
        vars: serde_json::Map<String, serde_json::Value>,
    ) -> TurnFut {
        let harness = self.harness.clone();
        Box::pin(async move {
            harness
                .prompt_from_template(&name, vars)
                .await
                .map(|_| None)
        })
    }

    pub(super) fn compaction_turn(&self, custom: Option<String>) -> TurnFut {
        let harness = self.harness.clone();
        Box::pin(async move {
            harness.force_compact(custom).await.map(|ran| {
                Some(if ran {
                    "compaction ran".to_string()
                } else {
                    "nothing to compact".to_string()
                })
            })
        })
    }

    pub(super) fn continue_turn(&self) -> TurnFut {
        let harness = self.harness.clone();
        Box::pin(async move { harness.continue_().await.map(|_| None) })
    }
}
