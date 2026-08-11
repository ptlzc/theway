//! Metrics/output accumulation listener for registered jobs.

use std::sync::Arc;

use theway_llm_provider::Message as PiMessage;

use crate::{AgentMessage, LoopEvent};

use super::{
    AgentJobEvent, AgentJobRegistry, agent_message_to_json, append_message, append_output,
};

/// Build a synchronous [`crate::agent::LoopSyncCallback`] that accumulates metrics + output
/// for a registered job. All internal operations are memory-only (lock + counter update +
/// non-blocking broadcast send), satisfying the <1µs sync-callback contract.
///
/// Attach to the sub-harness (`sub.agent().subscribe_sync(...)`) right after registering.
pub fn metrics_listener(
    registry: AgentJobRegistry,
    job_id: String,
) -> crate::agent::LoopSyncCallback {
    Arc::new(move |event| match event {
        LoopEvent::MessageUpdate {
            assistant_message_event:
                theway_llm_provider::AssistantMessageEvent::TextDelta { delta, .. },
            ..
        } => {
            let delta = delta.clone();
            registry.update(&job_id, |job| {
                job.chars = job.chars.saturating_add(delta.chars().count() as u64);
                append_output(job, &delta);
            });
            registry.emit(AgentJobEvent::Output {
                id: job_id.clone(),
                chunk: delta,
            });
        }
        LoopEvent::MessageEnd { message } => {
            let usage_tokens = match message {
                AgentMessage::Llm(PiMessage::Assistant(a)) => {
                    let usage = &a.usage;
                    let input = usage
                        .input
                        .saturating_add(usage.cache_read)
                        .saturating_add(usage.cache_write);
                    Some((input, usage.output))
                }
                _ => None,
            };
            registry.update(&job_id, |job| {
                append_message(job, &agent_message_to_json(message));
                if let Some((input, output)) = usage_tokens {
                    job.input_tokens = job.input_tokens.saturating_add(input);
                    job.output_tokens = job.output_tokens.saturating_add(output);
                }
            });
            if let Some(job) = registry.job(&job_id) {
                registry.emit(AgentJobEvent::Metrics {
                    id: job_id.clone(),
                    tps: job.tps(),
                    cps: job.cps(),
                    chars: job.chars,
                    tokens_in: job.input_tokens,
                    tokens_out: job.output_tokens,
                    tools_called: job.tools_called,
                    turn: job.turn,
                });
            }
        }
        LoopEvent::ToolExecutionStart { .. } => {
            registry.update(&job_id, |job| {
                job.tools_called = job.tools_called.saturating_add(1);
            });
        }
        LoopEvent::TurnStart => {
            registry.update(&job_id, |job| {
                job.turn = job.turn.saturating_add(1);
            });
        }
        _ => {}
    })
}
