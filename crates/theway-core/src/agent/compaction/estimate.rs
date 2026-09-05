//! Token/context estimation helpers for auto-compaction.

use serde::{Deserialize, Serialize};
use theway_llm_provider::Usage;

use super::super::session::session::SessionTreeEntry;
use super::compaction::CompactionSettings;
use crate::types::*;

// Token estimation
// ──────────────────────────────────────────────────────────────────────────────────────────

pub fn calculate_context_tokens(usage: &Usage) -> u64 {
    if usage.total_tokens > 0 {
        usage.total_tokens
    } else {
        usage.input + usage.output + usage.cache_read + usage.cache_write
    }
}

fn assistant_usage(msg: &AgentMessage) -> Option<&Usage> {
    let AgentMessage::Llm(PiMessage::Assistant(a)) = msg else {
        return None;
    };
    if matches!(
        a.stop_reason,
        theway_llm_provider::StopReason::Aborted | theway_llm_provider::StopReason::Error
    ) {
        return None;
    }
    if a.usage.total_tokens == 0
        && a.usage.input == 0
        && a.usage.output == 0
        && a.usage.cache_read == 0
        && a.usage.cache_write == 0
    {
        return None;
    }
    Some(&a.usage)
}

pub fn get_last_assistant_usage(entries: &[SessionTreeEntry]) -> Option<Usage> {
    for e in entries.iter().rev() {
        if let SessionTreeEntry::Message { message, .. } = e {
            if let Some(u) = assistant_usage(message) {
                return Some(u.clone());
            }
        }
    }
    None
}

/// Conservative char-class-aware text estimate: ~4 chars per token for ASCII, ~1 token per char
/// for non-ASCII (CJK and similar scripts tokenize close to one token per character). Rounds up.
pub fn estimate_text_tokens(s: &str) -> u64 {
    let mut ascii = 0u64;
    let mut non_ascii = 0u64;
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    ascii.div_ceil(4) + non_ascii
}

/// Conservative per-message estimate built on [`estimate_text_tokens`].
pub fn estimate_tokens(message: &AgentMessage) -> u64 {
    match message {
        AgentMessage::Llm(PiMessage::User(u)) => match &u.content {
            theway_llm_provider::UserContent::Text(s) => estimate_text_tokens(s),
            theway_llm_provider::UserContent::Blocks(blocks) => {
                blocks.iter().map(user_block_tokens).sum()
            }
        },
        AgentMessage::Llm(PiMessage::Assistant(a)) => {
            a.content.iter().map(content_block_tokens).sum()
        }
        AgentMessage::Llm(PiMessage::ToolResult(tr)) => {
            estimate_text_tokens(&tr.tool_name)
                + tr.content.iter().map(user_block_tokens).sum::<u64>()
        }
        AgentMessage::Custom(c) => {
            estimate_text_tokens(&c.role) + estimate_text_tokens(&c.payload.to_string())
        }
    }
}

fn user_block_tokens(b: &theway_llm_provider::UserContentBlock) -> u64 {
    match b {
        theway_llm_provider::UserContentBlock::Text(t) => estimate_text_tokens(&t.text),
        // Images are weighted as a flat 768 tokens — matches Anthropic's pricing approximation.
        // TS uses a similar heuristic.
        theway_llm_provider::UserContentBlock::Image(_) => 768,
    }
}

fn content_block_tokens(b: &theway_llm_provider::ContentBlock) -> u64 {
    match b {
        theway_llm_provider::ContentBlock::Text(t) => estimate_text_tokens(&t.text),
        theway_llm_provider::ContentBlock::Thinking(t) => estimate_text_tokens(&t.thinking),
        theway_llm_provider::ContentBlock::Image(_) => 768,
        theway_llm_provider::ContentBlock::ToolCall(tc) => {
            estimate_text_tokens(&tc.name)
                + estimate_text_tokens(&serde_json::Value::Object(tc.arguments.clone()).to_string())
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContextUsageEstimate {
    pub tokens: u64,
    pub usage_tokens: u64,
    pub trailing_tokens: u64,
    /// Index of the message that provided the usage block, or `None` when no assistant turn has
    /// finished yet.
    pub last_usage_index: Option<usize>,
}

pub fn estimate_context_tokens(messages: &[AgentMessage]) -> ContextUsageEstimate {
    let mut last_with_usage: Option<(usize, &Usage)> = None;
    for (i, m) in messages.iter().enumerate() {
        if let Some(u) = assistant_usage(m) {
            last_with_usage = Some((i, u));
        }
    }
    let Some((idx, usage)) = last_with_usage else {
        let total = messages.iter().map(estimate_tokens).sum();
        return ContextUsageEstimate {
            tokens: total,
            usage_tokens: 0,
            trailing_tokens: total,
            last_usage_index: None,
        };
    };
    let usage_tokens = calculate_context_tokens(usage);
    let trailing: u64 = messages[idx + 1..].iter().map(estimate_tokens).sum();
    ContextUsageEstimate {
        tokens: usage_tokens + trailing,
        usage_tokens,
        trailing_tokens: trailing,
        last_usage_index: Some(idx),
    }
}

pub fn should_compact(
    context_tokens: u64,
    context_window: u32,
    settings: &CompactionSettings,
) -> bool {
    if !settings.enabled {
        return false;
    }
    let window = context_window as u64;
    // Trigger auto-compaction at 80% of the context window so there's still
    // headroom for the summarizer LLM call and the next turn. Waiting until
    // `window - reserve_tokens` (≈87%+ with defaults) meant the next response
    // could overflow the window before compaction had a chance to run.
    let threshold = (window * 4) / 5;
    context_tokens > threshold
}
