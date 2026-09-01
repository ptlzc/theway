//! AgentSession — auto-retry wrapper around [`theway_core::AgentHarness`]. 1:1 port of
//! `packages/coding-agent/src/core/agent-session.ts` retry logic (`_isRetryableError` +
//! `_prepareRetry`).
//!
//! On a retryable LLM error the wrapper:
//! 1. Drops the failed assistant message from agent state and rewinds the active session leaf
//!    while keeping the append-only session log intact.
//! 2. Waits exponentially (`base_delay_ms * 2^(attempt-1)`, capped).
//! 3. Calls `harness.continue_()` to re-run.
//! 4. Up to `max_retries` times. After that, the error surfaces to the caller.
//!
//! Retryable patterns: `overloaded`, `rate.?limit`, `429`, `5xx`, network/connection errors,
//! `ended without`, `stream ended before message_stop`, `terminated`, `retry delay`. Mirrors
//! the TS regex.

use std::sync::Arc;

use once_cell::sync::Lazy;
use regex::Regex;
use theway_core::{AgentHarness, AgentMessage, AgentRunError, LoopListener, SessionTreeEntry};
use theway_llm_provider::{AssistantMessage as PiAssistantMessage, Message as PiMessage};

#[derive(Clone, Debug)]
pub struct RetrySettings {
    pub enabled: bool,
    pub max_retries: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
    /// When set and the primary model's retries exhaust on a retryable error, swap to this
    /// `(provider, model_id)` and retry once. Returns whatever the fallback says (success or
    /// definitive error). Each model is tried at most once per fallback chain — no looping.
    pub fallback_model: Option<(String, String)>,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_retries: 5,
            base_delay_ms: 1_000,
            max_delay_ms: 60_000,
            fallback_model: None,
        }
    }
}

/// Regex matching error messages we should retry. Compiled once.
static RETRYABLE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)overloaded|provider.?returned.?error|rate.?limit|too many requests|429|500|502|503|504|service.?unavailable|server.?error|internal.?error|network.?error|connection.?error|connection.?refused|connection.?lost|websocket.?closed|websocket.?error|other side closed|fetch failed|upstream.?connect|reset before headers|socket hang up|ended without|stream ended before message_stop|http2 request did not get a response|timed? out|timeout|terminated|retry delay",
    )
    .expect("retry regex")
});

pub fn is_retryable_error(err_message: &str) -> bool {
    RETRYABLE_RE.is_match(err_message)
}

/// Lightweight wrapper. Holds the harness + retry settings; not a deep clone of TS
/// `AgentSession` (no extension orchestration, no event-bus fan-out — just retry).
pub struct AgentSession {
    harness: Arc<AgentHarness>,
    settings: RetrySettings,
}

impl AgentSession {
    pub fn new(harness: Arc<AgentHarness>, settings: RetrySettings) -> Self {
        Self { harness, settings }
    }

    #[allow(dead_code)] // public API for embedders; not used by the binary itself.
    pub fn harness(&self) -> &AgentHarness {
        &self.harness
    }

    /// Subscribe to underlying lifecycle events. Useful for UI listeners.
    #[allow(dead_code)] // public API for embedders; not used by the binary itself.
    pub fn subscribe(&self, listener: LoopListener) -> impl FnOnce() {
        self.harness.subscribe(listener)
    }

    /// Prompt with retry. Same signature as `AgentHarness::prompt` but loops on retryable LLM
    /// errors with exponential backoff. If a `fallback_model` is configured and the primary
    /// run exhausts retries, the model is swapped exactly once and the loop restarts.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentRunError> {
        let text = text.into();
        let mut attempt: u32 = 0;
        let mut fallback_used = false;
        loop {
            let r = if attempt == 0 {
                self.harness.prompt(text.clone()).await
            } else {
                self.harness.continue_().await
            };
            let err = match r {
                Ok(()) => match self.assistant_error_message(&self.last_assistant()) {
                    Some(error_message) => AgentRunError::Other(error_message),
                    None => return Ok(()),
                },
                Err(e) => e,
            };
            // Successful prompt() can still leave a synthesized error assistant message
            // (provider stream encoded the error). Re-evaluate via retry policy.

            if !self.settings.enabled {
                return Err(err);
            }

            if !is_retryable_error(&err.to_string()) {
                return Err(err);
            }

            if attempt >= self.settings.max_retries {
                // Exhausted retries on the current model. If a fallback is configured and we
                // haven't already used it, swap and restart from attempt=0.
                if let Some((provider, model_id)) = &self.settings.fallback_model {
                    if !fallback_used {
                        fallback_used = true;
                        if let Some(m) = theway_llm_provider::get_model(
                            &theway_llm_provider::Provider::from(provider.as_str()),
                            model_id,
                        ) {
                            self.rewind_failed_assistant().await?;
                            if let Err(e) = self.harness.set_model(m).await {
                                return Err(AgentRunError::Other(format!(
                                    "fallback set_model failed: {e}"
                                )));
                            }
                            attempt = 0;
                            continue;
                        }
                    }
                }
                return Err(err);
            }

            attempt += 1;
            let delay_ms = backoff_ms(
                attempt,
                self.settings.base_delay_ms,
                self.settings.max_delay_ms,
            );
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

            // Drop the failed assistant message from agent state so continue_() doesn't replay
            // a context that ends in an error.
            self.rewind_failed_assistant().await?;
        }
    }

    fn last_assistant(&self) -> Option<PiAssistantMessage> {
        let s = self.harness.agent().state();
        for m in s.messages.iter().rev() {
            if let AgentMessage::Llm(PiMessage::Assistant(a)) = m {
                return Some(a.clone());
            }
        }
        None
    }

    fn assistant_error_message(&self, a: &Option<PiAssistantMessage>) -> Option<String> {
        let Some(a) = a else { return None };
        if a.stop_reason != theway_llm_provider::StopReason::Error {
            return None;
        }
        a.error_message
            .clone()
            .or_else(|| Some("assistant stopped with an error".to_string()))
    }

    async fn rewind_failed_assistant(&self) -> Result<(), AgentRunError> {
        let mut s = self.harness.agent().state();
        while let Some(last) = s.messages.last() {
            if matches!(last, AgentMessage::Llm(PiMessage::Assistant(a)) if a.stop_reason == theway_llm_provider::StopReason::Error)
            {
                s.messages.pop();
            } else {
                break;
            }
        }
        drop(s);

        let session = self.harness.session();
        let Some(leaf_id) = session
            .leaf_id()
            .await
            .map_err(|e| AgentRunError::Other(format!("session retry leaf lookup: {e}")))?
        else {
            return Ok(());
        };
        let Some(SessionTreeEntry::Message {
            parent_id,
            message: AgentMessage::Llm(PiMessage::Assistant(a)),
            ..
        }) = session
            .get_entry(&leaf_id)
            .await
            .map_err(|e| AgentRunError::Other(format!("session retry leaf entry lookup: {e}")))?
        else {
            return Ok(());
        };
        if a.stop_reason == theway_llm_provider::StopReason::Error {
            session
                .move_to(parent_id.as_deref(), None)
                .await
                .map_err(|e| AgentRunError::Other(format!("session retry rewind: {e}")))?;
        }
        Ok(())
    }
}

fn backoff_ms(attempt: u32, base: u64, max: u64) -> u64 {
    let exponent = attempt.saturating_sub(1).min(10);
    let n = (base as u128) << exponent;
    n.min(max as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_core::{AgentHarnessOptions, MemorySessionStorage, Session, SessionStorage};
    use theway_llm_provider::{
        AssistantMessage, AssistantMessageEvent, AssistantMessageEventStream, AssistantRole,
        ContentBlock, DoneReason, ModelCost, StopReason, Usage,
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
            cost: ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        }
    }

    fn assistant(
        text: &str,
        stop_reason: StopReason,
        error_message: Option<&str>,
    ) -> AssistantMessage {
        AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![ContentBlock::text(text)],
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            model: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason,
            error_message: error_message.map(str::to_string),
            timestamp: 0,
        }
    }

    fn stream_fn_with(
        responses: Arc<tokio::sync::Mutex<Vec<AssistantMessage>>>,
    ) -> theway_core::StreamFn {
        Arc::new(move |_, _, _| {
            let (stream, mut sender) = AssistantMessageEventStream::new();
            let responses = responses.clone();
            tokio::spawn(async move {
                let msg = responses.lock().await.remove(0);
                sender.push(AssistantMessageEvent::Start {
                    partial: msg.clone(),
                });
                let reason = match msg.stop_reason {
                    StopReason::ToolUse => DoneReason::ToolUse,
                    StopReason::Length => DoneReason::Length,
                    _ => DoneReason::Stop,
                };
                sender.push(AssistantMessageEvent::Done {
                    reason,
                    message: msg,
                });
            });
            stream
        })
    }

    #[test]
    fn retryable_patterns_match_ts_regex() {
        assert!(is_retryable_error("overloaded_error"));
        assert!(is_retryable_error(
            "Provider returned error: 429 Too Many Requests"
        ));
        assert!(is_retryable_error("rate limit exceeded"));
        assert!(is_retryable_error("HTTP 503 Service Unavailable"));
        assert!(is_retryable_error("websocket closed"));
        assert!(is_retryable_error("stream ended before message_stop"));
        assert!(is_retryable_error("socket hang up"));
        assert!(is_retryable_error("reset before headers"));
        assert!(!is_retryable_error("bad request: missing field"));
        assert!(!is_retryable_error("Unauthorized"));
        assert!(!is_retryable_error("model not found"));
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_ms(1, 1000, 60_000), 1000);
        assert_eq!(backoff_ms(2, 1000, 60_000), 2000);
        assert_eq!(backoff_ms(9, 1000, 60_000), 60_000);
    }

    #[tokio::test]
    async fn retry_rewinds_failed_assistant_out_of_active_session_branch() {
        let storage = Arc::new(MemorySessionStorage::new());
        let session = Session::new(storage as Arc<dyn SessionStorage>);
        let responses = Arc::new(tokio::sync::Mutex::new(vec![
            assistant("temporary failure", StopReason::Error, Some("HTTP 503")),
            assistant("ok", StopReason::Stop, None),
        ]));

        let mut opts = AgentHarnessOptions::new(Some(faux_model()), session.clone());
        opts.stream_fn = Some(stream_fn_with(responses));
        let harness = Arc::new(AgentHarness::new(opts));
        let runner = AgentSession::new(
            harness,
            RetrySettings {
                base_delay_ms: 0,
                max_delay_ms: 0,
                max_retries: 1,
                ..RetrySettings::default()
            },
        );

        runner.prompt("hi").await.unwrap();

        let entries = session.entries().await.unwrap();
        assert!(entries.iter().any(|e| matches!(
            e,
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(PiMessage::Assistant(a)),
                ..
            } if a.stop_reason == StopReason::Error
        )));

        let active = session.build_context().await.unwrap();
        assert!(!active.messages.iter().any(|m| matches!(
            m,
            AgentMessage::Llm(PiMessage::Assistant(a)) if a.stop_reason == StopReason::Error
        )));
        assert!(active.messages.iter().any(|m| matches!(
            m,
            AgentMessage::Llm(PiMessage::Assistant(a))
                if a.stop_reason == StopReason::Stop
        )));
    }
}

#[cfg(test)]
mod agent_session_mirrored_tests {
    //! Mirrored tests live in `tests/agent_session/`; wrapped because the
    //! top-level `mod tests` slot is already used by the inline tests.
    tests_bridge_macro::tests_bridge!("agent_session");
}
