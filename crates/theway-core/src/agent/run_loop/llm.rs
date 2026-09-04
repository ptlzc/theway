//! LLM streaming call for the agent loop.

use std::sync::Arc;

use futures::StreamExt;
use theway_llm_provider::{
    AssistantMessage as PiAssistantMessage, AssistantMessageEvent, Context as PiContext,
    Message as PiMessage, SimpleStreamOptions,
};
use tokio_util::sync::CancellationToken;

use crate::agent::model_request::{NormalizedGenerationOptions, NormalizedModelRequestDraft};
use crate::agent::{AgentInner, AgentRunError};
use crate::observability::{
    ErrorCategory, OperationDetail, OperationOutcome, OperationScope, RuntimeMeasurements,
};
use crate::types::*;

use super::utils::emit;

pub(super) struct ModelCallResult {
    pub(super) message: PiAssistantMessage,
    pub(super) executable_tools: Vec<Arc<dyn AgentTool>>,
}

impl std::fmt::Debug for ModelCallResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelCallResult")
            .field("message", &self.message)
            .field(
                "executable_tools",
                &self
                    .executable_tools
                    .iter()
                    .map(|tool| tool.definition().name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl std::ops::Deref for ModelCallResult {
    type Target = PiAssistantMessage;

    fn deref(&self) -> &Self::Target {
        &self.message
    }
}

pub(super) async fn call_llm(
    inner: &Arc<AgentInner>,
    cancel: &CancellationToken,
    turn_cancel: &CancellationToken,
) -> Result<ModelCallResult, AgentRunError> {
    let (system_prompt, agent_messages, tools, model, thinking_level) = {
        let g = inner.state.lock();
        let model = g.model.clone().ok_or_else(|| {
            AgentRunError::Other("Agent has no model set; assign state.model first".into())
        })?;
        (
            g.system_prompt.clone(),
            g.messages.clone(),
            g.tools.clone(),
            model,
            g.thinking_level,
        )
    };
    let active_turn = *inner.active_turn_operation.lock();
    let context = active_turn
        .map(|(_, turn)| inner.options.observation_context.with_turn(turn))
        .unwrap_or_else(|| inner.options.observation_context.clone());
    let scope = OperationScope::start(
        Arc::clone(&inner.options.observer),
        active_turn.map(|(id, _)| id),
        context,
        OperationDetail::LlmRequest {
            provider: model.provider.0.clone(),
            model: model.id.clone(),
        },
    );

    let result = async {
        // `transform_context` runs before convert_to_llm so callers can prune / inject ephemeral
        // context without mutating persisted state.
        let agent_messages = if let Some(transform) = inner.options.transform_context.clone() {
            transform(agent_messages, cancel.clone()).await
        } else {
            agent_messages
        };

        let messages = inner.convert_to_llm(&agent_messages);
        let visible_tools: Vec<theway_llm_provider::Tool> =
            tools.iter().map(|t| t.definition().clone()).collect();
        let executable_tool_names = visible_tools.iter().map(|tool| tool.name.clone()).collect();
        let base_request = NormalizedModelRequestDraft {
            provider: model.provider.0.clone(),
            model: model.id.clone(),
            system_instructions: if system_prompt.is_empty() {
                None
            } else {
                Some(system_prompt)
            },
            messages,
            visible_tools,
            executable_tool_names,
            generation_options: NormalizedGenerationOptions {
                reasoning: thinking_level.and_then(|level| level.to_theway_llm_provider()),
                ..Default::default()
            },
        };

        let transformed = if let Some(transform) = inner.options.transform_model_request.clone() {
            transform(base_request.clone(), cancel.clone()).await
        } else {
            base_request.clone()
        };
        let request = if transformed
            .validate_replacement(&base_request, model.max_tokens)
            .is_ok()
        {
            transformed
        } else {
            base_request
        };
        let executable_tools = request
            .executable_tool_names
            .iter()
            .filter_map(|name| {
                tools
                    .iter()
                    .find(|tool| tool.definition().name == *name)
                    .cloned()
            })
            .collect::<Vec<_>>();
        let context = PiContext {
            system_prompt: request.system_instructions,
            messages: request.messages,
            tools: (!request.visible_tools.is_empty()).then_some(request.visible_tools),
        };

        let prefix_estimate = inner.context_cache.lock().estimate(
            inner.options.session_id.as_deref(),
            &model.provider.0,
            &model.id,
            &context,
        );

        let stream_fn = inner
            .options
            .stream_fn
            .clone()
            .unwrap_or_else(default_stream_fn);
        let mut options = SimpleStreamOptions::default();
        if let Some(resolve) = &inner.options.get_api_key {
            options.base.api_key = resolve(&model.provider.0);
        }
        if let Some(sid) = &inner.options.session_id {
            options.base.session_id = Some(sid.clone());
        }
        options.base.abort = Some(cancel.clone());
        options.base.request_interceptor = inner.options.provider_request_interceptor.clone();
        options.base.temperature = request.generation_options.temperature;
        options.base.max_tokens = request.generation_options.max_tokens;
        options.reasoning = request.generation_options.reasoning;
        options.thinking_budgets = request.generation_options.thinking_budgets;

        let mut stream = stream_fn(&model, &context, Some(&options));
        let mut last_message: Option<PiAssistantMessage> = None;
        loop {
            // Race the stream's next event against the cancellation token. Polling order is
            // biased toward cancellation so a Ctrl-C arriving mid-stall doesn't have to wait
            // for the next provider event to flush before we bail out. Closes #18.
            let ev = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(AgentRunError::Other("aborted".into()));
                }
                _ = turn_cancel.cancelled() => {
                    return Err(AgentRunError::TurnInterrupted);
                }
                next = stream.next() => match next {
                    Some(ev) => ev,
                    None => break,
                }
            };
            match &ev {
                AssistantMessageEvent::Start { partial } => {
                    last_message = Some(partial.clone());
                    let m = AgentMessage::Llm(PiMessage::Assistant(partial.clone()));
                    emit(
                        inner,
                        LoopEvent::MessageStart { message: m.clone() },
                        cancel,
                    )
                    .await;
                    inner.state.lock().streaming_message = Some(m);
                }
                AssistantMessageEvent::TextDelta { partial, .. }
                | AssistantMessageEvent::TextEnd { partial, .. }
                | AssistantMessageEvent::ThinkingDelta { partial, .. }
                | AssistantMessageEvent::ThinkingEnd { partial, .. }
                | AssistantMessageEvent::ToolCallDelta { partial, .. }
                | AssistantMessageEvent::ToolCallEnd { partial, .. } => {
                    last_message = Some(partial.clone());
                    let m = AgentMessage::Llm(PiMessage::Assistant(partial.clone()));
                    inner.state.lock().streaming_message = Some(m.clone());
                    emit(
                        inner,
                        LoopEvent::MessageUpdate {
                            message: m,
                            assistant_message_event: ev.clone(),
                        },
                        cancel,
                    )
                    .await;
                }
                AssistantMessageEvent::Done { message, .. } => {
                    last_message = Some(message.clone());
                }
                AssistantMessageEvent::Error { error, .. } => {
                    // last_message would be overwritten by `return Err` below; don't bother.
                    let msg = error.error_message.clone().unwrap_or_default();
                    inner.state.lock().streaming_message = None;
                    return Err(AgentRunError::Other(msg));
                }
                _ => {}
            }
        }
        inner.state.lock().streaming_message = None;
        let mut message = last_message
            .ok_or_else(|| AgentRunError::Other("LLM stream produced no message".into()))?;
        // Issue #105: `usage.input` is the TOTAL input token count (prompt
        // tokens already include cached reads on OpenAI/DeepSeek), and
        // `usage.cache_read` is the cached subset of it. Adding them again
        // double-counted cache reads and pinned the hit rate near 50% for
        // high-hit sessions.
        let total_input_tokens = message.usage.input;
        let prefix_result = inner
            .context_cache
            .lock()
            .finalize(&prefix_estimate, total_input_tokens);
        message.usage.prefix_hit_tokens = Some(prefix_result.prefix_hit_tokens);
        message.usage.prefix_cache_hit_rate = prefix_result.prefix_cache_hit_rate;
        message.usage.provider_cache_hit_rate =
            if total_input_tokens > 0 && message.usage.cache_read > 0 {
                Some(message.usage.cache_read as f64 / total_input_tokens as f64)
            } else {
                None
            };
        Ok(ModelCallResult {
            message,
            executable_tools,
        })
    }
    .await;

    let (outcome, category, measurements) = match &result {
        Ok(call) => (
            OperationOutcome::Succeeded,
            None,
            RuntimeMeasurements {
                input_tokens: call.message.usage.input,
                output_tokens: call.message.usage.output,
                cache_read_tokens: call.message.usage.cache_read,
                cache_write_tokens: call.message.usage.cache_write,
                ..Default::default()
            },
        ),
        Err(AgentRunError::TurnInterrupted) => (
            OperationOutcome::Interrupted,
            Some(ErrorCategory::Cancellation),
            RuntimeMeasurements::default(),
        ),
        Err(_) if cancel.is_cancelled() => (
            OperationOutcome::Cancelled,
            Some(ErrorCategory::Cancellation),
            RuntimeMeasurements::default(),
        ),
        Err(_) => (
            OperationOutcome::Failed,
            Some(ErrorCategory::Provider),
            RuntimeMeasurements::default(),
        ),
    };
    scope.finish(outcome, category, measurements);
    result
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/run_loop/llm");

#[cfg(test)]
mod llm_linecov_tests {
    tests_bridge_macro::tests_bridge!("agent/run_loop/llm/linecov");
}
