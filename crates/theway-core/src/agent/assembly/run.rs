use theway_llm_provider::{ImageContent, Message as PiMessage};

use crate::agent::AgentRunError;
use crate::types::AgentMessage;

use super::runtime_extensions::InputTransformOutcome;
use super::session::{finish_persisted_run, make_session_listener};
use super::{AgentHarness, OnTurnEndContext, SessionEvent, TurnEndAction};

const RUNTIME_EXTENSION_FOLLOW_UP_CHAIN_CAP: u32 = 16;

impl AgentHarness {
    /// Prompt the agent with text. Runs auto-compaction first and persists results.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<(), AgentRunError> {
        let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }));
        self.prompt_with_message(user_message).await
    }

    /// Prompt with text and images.
    pub async fn prompt_with_images(
        &self,
        text: impl Into<String>,
        images: Vec<ImageContent>,
    ) -> Result<(), AgentRunError> {
        let mut blocks: Vec<theway_llm_provider::UserContentBlock> = images
            .into_iter()
            .map(theway_llm_provider::UserContentBlock::Image)
            .collect();
        let text = text.into();
        if !text.is_empty() {
            blocks.insert(0, theway_llm_provider::UserContentBlock::text(text));
        }
        let user_message = AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Blocks(blocks),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }));
        self.prompt_with_message(user_message).await
    }

    async fn prompt_with_message(&self, message: AgentMessage) -> Result<(), AgentRunError> {
        self.runtime_extensions
            .reject_reentrant_operation()
            .map_err(|error| AgentRunError::Other(error.message))?;
        self.ensure_session_start_emitted().await;
        let message = match self.runtime_extensions.transform_input(message).await {
            InputTransformOutcome::Run(message) => message,
            InputTransformOutcome::Handled(outcome) => {
                self.emit_harness_event(SessionEvent::ExtensionCommandOutcome { outcome });
                return Ok(());
            }
        };
        self.check_budget_cap()?;
        self.run_auto_compaction().await?;
        let last_user_prompt = extract_user_prompt_text(&message);
        self.run_turn_with_continuation(Some(message), last_user_prompt)
            .await
    }

    pub async fn continue_(&self) -> Result<(), AgentRunError> {
        self.runtime_extensions
            .reject_reentrant_operation()
            .map_err(|error| AgentRunError::Other(error.message))?;
        self.ensure_session_start_emitted().await;
        self.check_budget_cap()?;
        self.run_auto_compaction().await?;
        let last_user_prompt = self.last_user_text_from_state();
        self.run_turn_with_continuation(None, last_user_prompt)
            .await
    }

    async fn run_turn_with_continuation(
        &self,
        first_message: Option<AgentMessage>,
        last_user_prompt: Option<String>,
    ) -> Result<(), AgentRunError> {
        let mut continuation_count = 0_u32;
        let mut extension_follow_up_count = 0_u32;
        let mut pending_user_message = first_message;
        let mut is_first_iteration = true;
        let mut last_user_prompt = last_user_prompt;

        loop {
            self.runtime_extensions
                .begin_run()
                .map_err(|error| AgentRunError::Other(error.message))?;
            let before_run = self.runtime_extensions.before_run().await;
            if !before_run.messages.is_empty() {
                if let Err(error) = self
                    .session
                    .append_messages(before_run.messages.clone())
                    .await
                {
                    self.runtime_extensions.settle_run();
                    return Err(AgentRunError::Other(format!(
                        "persist before-run messages: {error}"
                    )));
                }
                self.agent
                    .state()
                    .messages
                    .extend(before_run.messages.clone());
            }
            let previous_system_prompt = before_run.system_prompt.map(|system_prompt| {
                let mut state = self.agent.state();
                std::mem::replace(&mut state.system_prompt, system_prompt)
            });
            let (listener, persist_errors) = make_session_listener(self.session.clone());
            let unsubscribe = self.agent.subscribe(listener);
            let (extension_listener, cancelled) = self.runtime_extensions.make_loop_listener();
            let unsubscribe_extensions = self.agent.subscribe(extension_listener);
            let result = if is_first_iteration {
                match pending_user_message.take() {
                    Some(message) => self.agent.prompt(message).await,
                    None => self.agent.continue_().await,
                }
            } else {
                let message = pending_user_message.take().expect(
                    "continuation iteration must have a pending user message from the hook",
                );
                self.agent.prompt(message).await
            };
            unsubscribe_extensions();
            unsubscribe();
            let result = finish_persisted_run(result, persist_errors);
            let was_cancelled = cancelled.load(std::sync::atomic::Ordering::Acquire);
            let outcome = if was_cancelled {
                "cancelled"
            } else if result.is_ok() {
                "success"
            } else {
                "error"
            };
            self.runtime_extensions
                .observe_run(
                    theway_contract::extension::ExtensionLifecycleEvent::RunEnded,
                    serde_json::json!({"outcome": outcome}),
                    was_cancelled,
                )
                .await;
            if result.is_err() {
                self.runtime_extensions
                    .observe_run(
                        theway_contract::extension::ExtensionLifecycleEvent::RunError,
                        serde_json::json!({
                            "category": if was_cancelled { "cancelled" } else { "runtime" },
                        }),
                        was_cancelled,
                    )
                    .await;
            }
            self.runtime_extensions
                .observe_run(
                    theway_contract::extension::ExtensionLifecycleEvent::RunSettled,
                    serde_json::json!({"outcome": outcome}),
                    was_cancelled,
                )
                .await;
            self.runtime_extensions.settle_run();
            if let Some(previous_system_prompt) = previous_system_prompt {
                self.agent.state().system_prompt = previous_system_prompt;
            }
            result?;
            is_first_iteration = false;

            if let Some(message) = self.runtime_extensions.take_follow_up() {
                if extension_follow_up_count >= RUNTIME_EXTENSION_FOLLOW_UP_CHAIN_CAP {
                    return Err(AgentRunError::Other(format!(
                        "runtime extension follow-up chain exceeded {} runs",
                        RUNTIME_EXTENSION_FOLLOW_UP_CHAIN_CAP
                    )));
                }
                extension_follow_up_count = extension_follow_up_count.saturating_add(1);
                last_user_prompt = extract_user_prompt_text(&message);
                pending_user_message = Some(message);
                self.check_budget_cap()?;
                self.run_auto_compaction().await?;
                continue;
            }

            let Some(hook) = self.on_turn_end.clone() else {
                return Ok(());
            };
            if continuation_count >= self.turn_continuation_cap {
                let reason = format!(
                    "continuation cap reached: {} >= {}",
                    continuation_count, self.turn_continuation_cap
                );
                self.record_turn_end_decision(
                    "budget_limited",
                    continuation_count,
                    Some(reason),
                    None,
                    None,
                )
                .await;
                return Ok(());
            }

            let context = OnTurnEndContext {
                transcript: self.agent.state().messages.clone(),
                continuation_count,
                last_user_prompt: last_user_prompt.clone(),
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            *self.active_hook_cancel.lock() = Some(cancel.clone());
            let decision = hook(context, cancel).await;
            *self.active_hook_cancel.lock() = None;

            match decision.action {
                TurnEndAction::Noop => return Ok(()),
                TurnEndAction::Stop => {
                    self.record_turn_end_decision(
                        "stop",
                        continuation_count,
                        None,
                        None,
                        decision.payload,
                    )
                    .await;
                    return Ok(());
                }
                TurnEndAction::Pause { reason } => {
                    self.record_turn_end_decision(
                        "pause",
                        continuation_count,
                        Some(reason),
                        None,
                        decision.payload,
                    )
                    .await;
                    return Ok(());
                }
                TurnEndAction::Continue { prompt } => {
                    continuation_count = continuation_count.saturating_add(1);
                    self.record_turn_end_decision(
                        "continue",
                        continuation_count,
                        None,
                        Some(preview_for_banner(&prompt, 80)),
                        decision.payload,
                    )
                    .await;
                    self.check_budget_cap()?;
                    self.run_auto_compaction().await?;
                    let user_message =
                        AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
                            role: theway_llm_provider::UserRole::User,
                            content: theway_llm_provider::UserContent::Text(prompt.clone()),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        }));
                    last_user_prompt = Some(prompt);
                    pending_user_message = Some(user_message);
                }
            }
        }
    }

    pub(super) fn check_budget_cap(&self) -> Result<(), AgentRunError> {
        if let Some(cap) = self.budget_cap_usd {
            let total = self.cost.snapshot().tokens.cost.total;
            if total >= cap {
                return Err(AgentRunError::Other(format!(
                    "budget cap reached: ${total:.4} >= ${cap:.4}. Reset with /cost reset or raise budget_cap_usd.",
                )));
            }
        }
        Ok(())
    }

    pub(super) fn last_user_text_from_state(&self) -> Option<String> {
        let state = self.agent.state();
        state
            .messages
            .iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Llm(PiMessage::User(user)) => extract_user_message_text(user),
                _ => None,
            })
    }

    pub(super) async fn record_turn_end_decision(
        &self,
        decision: &'static str,
        continuation_count: u32,
        reason: Option<String>,
        next_prompt_preview: Option<String>,
        payload: Option<serde_json::Value>,
    ) {
        let data = serde_json::json!({
            "decision": decision,
            "continuation_count": continuation_count,
            "reason": reason,
            "next_prompt_preview": next_prompt_preview,
            "payload": payload.unwrap_or(serde_json::Value::Null),
        });
        if let Err(error) = self
            .session
            .append_custom("turn_end_decision", Some(data))
            .await
        {
            self.emit_harness_event(SessionEvent::PersistenceError {
                context: "turn_end_decision".into(),
                message: format!("turn_end_decision append failed: {:?}", error.code),
            });
        }
        self.emit_harness_event(SessionEvent::TurnDecision {
            decision,
            continuation_count,
            reason,
            next_prompt_preview,
        });
    }
}

pub(super) fn preview_for_banner(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut output: String = text.chars().take(max_chars).collect();
    output.push('…');
    output
}

pub(super) fn extract_user_message_text(user: &theway_llm_provider::UserMessage) -> Option<String> {
    match &user.content {
        theway_llm_provider::UserContent::Text(text) => (!text.is_empty()).then(|| text.clone()),
        theway_llm_provider::UserContent::Blocks(blocks) => {
            let mut output = String::new();
            for block in blocks {
                if let theway_llm_provider::UserContentBlock::Text(text) = block {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&text.text);
                }
            }
            (!output.is_empty()).then_some(output)
        }
    }
}

pub(super) fn extract_user_prompt_text(message: &AgentMessage) -> Option<String> {
    match message {
        AgentMessage::Llm(PiMessage::User(user)) => extract_user_message_text(user),
        _ => None,
    }
}
