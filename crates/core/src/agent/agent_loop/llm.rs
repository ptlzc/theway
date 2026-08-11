//! LLM streaming call for the agent loop.

use std::sync::Arc;

use futures::StreamExt;
use theway_llm_provider::{
    AssistantMessage as PiAssistantMessage, AssistantMessageEvent, Context as PiContext,
    Message as PiMessage, SimpleStreamOptions,
};
use tokio_util::sync::CancellationToken;

use crate::agent::{AgentInner, AgentRunError};
use crate::types::*;

use super::utils::emit;

pub(super) async fn call_llm(
    inner: &Arc<AgentInner>,
    cancel: &CancellationToken,
    turn_cancel: &CancellationToken,
) -> Result<PiAssistantMessage, AgentRunError> {
    let (system_prompt, agent_messages, tools, model) = {
        let g = inner.state.lock();
        let model = g.model.clone().ok_or_else(|| {
            AgentRunError::Other("Agent has no model set; assign state.model first".into())
        })?;
        (
            g.system_prompt.clone(),
            g.messages.clone(),
            g.tools.clone(),
            model,
        )
    };

    // `transform_context` runs before convert_to_llm so callers can prune / inject ephemeral
    // context without mutating persisted state.
    let agent_messages = if let Some(transform) = inner.options.transform_context.clone() {
        transform(agent_messages, cancel.clone()).await
    } else {
        agent_messages
    };

    let messages = inner.convert_to_llm(&agent_messages);
    let pi_tools: Vec<theway_llm_provider::Tool> =
        tools.iter().map(|t| t.definition().clone()).collect();
    let context = PiContext {
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        messages,
        tools: if pi_tools.is_empty() {
            None
        } else {
            Some(pi_tools)
        },
    };

    let stream_fn = inner
        .options
        .stream_fn
        .clone()
        .unwrap_or_else(default_stream_fn);
    let mut options = SimpleStreamOptions::default();
    if let Some(sid) = &inner.options.session_id {
        options.base.session_id = Some(sid.clone());
    }
    options.base.abort = Some(cancel.clone());
    if let Some(level) = inner
        .state
        .lock()
        .thinking_level
        .and_then(|l| l.to_theway_llm_provider())
    {
        options.reasoning = Some(level);
    }

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
                    AgentEvent::MessageStart { message: m.clone() },
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
                    AgentEvent::MessageUpdate {
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
    last_message.ok_or_else(|| AgentRunError::Other("LLM stream produced no message".into()))
}
