//! Auto-compaction. Partial 1:1 port of
//! `packages/agent/src/harness/compaction/compaction.ts` (~755 lines).
//!
//! Implemented:
//! - `CompactionSettings` + `DEFAULT_COMPACTION_SETTINGS`
//! - `calculate_context_tokens` / `estimate_tokens` / `estimate_context_tokens`
//! - `should_compact`
//! - `find_turn_start_index` / `find_cut_point` (turn-boundary-safe)
//! - `SUMMARIZATION_SYSTEM_PROMPT`
//! - `generate_summary` (calls the StreamFn to summarize a message prefix)
//! - `prepare_compaction` (decides cut point + assembles entries to summarize)
//! - `compact` (the orchestration entry point)
//!
//! TODO:
//! - more nuanced char→token weights for image/tool blocks (currently flat)
//! - `serialize_conversation` formatting parity with TS (used inside summarization prompts)

use futures::StreamExt;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use theway_llm_provider::{
    AssistantMessage, AssistantMessageEvent, Context as PiContext, Message as PiMessage, Model,
    SimpleStreamOptions, Usage,
};
use tokio_util::sync::CancellationToken;

use super::super::super::types::default_stream_fn;
use super::super::super::types::*;
use super::super::session::session::SessionTreeEntry;
use super::algorithm::{CompactAlgorithm, SummarizeRequest, SummaryOutcome};

// ──────────────────────────────────────────────────────────────────────────────────────────
// Settings
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Default `CompactionSettings.algorithm` — the builtin strategy.
pub fn default_compaction_algorithm() -> String {
    "builtin".to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompactionSettings {
    /// Enable automatic compaction decisions.
    pub enabled: bool,
    /// Tokens reserved for summary prompt + output.
    pub reserve_tokens: u32,
    /// Approximate recent-context tokens to keep after compaction.
    pub keep_recent_tokens: u32,
    /// Compaction algorithm to use: `"builtin"` (default) or the name of a custom
    /// algorithm (e.g. a TS extension under `.theway/extensions/compaction/<name>.ts`).
    #[serde(default = "default_compaction_algorithm")]
    pub algorithm: String,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        DEFAULT_COMPACTION_SETTINGS.clone()
    }
}

pub static DEFAULT_COMPACTION_SETTINGS: Lazy<CompactionSettings> =
    Lazy::new(|| CompactionSettings {
        enabled: true,
        reserve_tokens: 16_384,
        keep_recent_tokens: 20_000,
        algorithm: default_compaction_algorithm(),
    });

// ──────────────────────────────────────────────────────────────────────────────────────────
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

// ──────────────────────────────────────────────────────────────────────────────────────────
// Cut-point detection (turn-boundary safe)
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Walk backward from `entry_index` until we hit a user-message entry — the turn boundary.
/// Returns that user-message's index. If no user message exists in `entries[start_index..=entry_index]`,
/// returns `start_index`.
pub fn find_turn_start_index(
    entries: &[SessionTreeEntry],
    entry_index: usize,
    start_index: usize,
) -> usize {
    let upper = entry_index.min(entries.len().saturating_sub(1));
    let mut i = upper as isize;
    while i >= start_index as isize {
        let idx = i as usize;
        if let SessionTreeEntry::Message { message, .. } = &entries[idx] {
            if matches!(message, AgentMessage::Llm(PiMessage::User(_))) {
                return idx;
            }
        }
        i -= 1;
    }
    start_index
}

#[derive(Clone, Debug)]
pub struct CutPointResult {
    /// Index in `entries` such that entries[..cut_index] are summarized and entries[cut_index..]
    /// are kept verbatim.
    pub cut_index: usize,
    /// id of the first kept entry, used in the `compaction` record.
    pub first_kept_entry_id: Option<String>,
}

/// Find a safe cut point keeping at least `keep_recent_tokens` of trailing context. Always lands
/// on a turn boundary.
pub fn find_cut_point(
    entries: &[SessionTreeEntry],
    settings: &CompactionSettings,
) -> CutPointResult {
    if entries.is_empty() {
        return CutPointResult {
            cut_index: 0,
            first_kept_entry_id: None,
        };
    }
    // Walk backward summing tokens until we've kept `keep_recent_tokens`, then back up to the
    // turn boundary above that.
    let mut acc: u64 = 0;
    let mut target = entries.len();
    for (i, entry) in entries.iter().enumerate().rev() {
        if let SessionTreeEntry::Message { message, .. } = entry {
            acc += estimate_tokens(message);
        }
        if acc >= settings.keep_recent_tokens as u64 {
            target = i;
            break;
        }
    }
    let cut = find_turn_start_index(entries, target, 0);
    let first_kept_entry_id = entries.get(cut).map(|e| e.id().to_string());
    CutPointResult {
        cut_index: cut,
        first_kept_entry_id,
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Summarization
// ──────────────────────────────────────────────────────────────────────────────────────────

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured summary preserving the user's intent, the files and topics discussed, decisions made, and any work still in progress. Be concise but thorough; the assistant will rely on your summary instead of replaying the dropped messages.";
const DEFAULT_SUMMARY_PROMPT_TOKEN_BUDGET: u64 = 64_000;

/// Synchronous helper used by the LLM-backed `generate_summary`. Serialize a message list into a
/// compact text dump for the summarizer prompt.
pub fn serialize_conversation(messages: &[AgentMessage]) -> String {
    let mut out = String::new();
    for m in messages {
        match m {
            AgentMessage::Llm(PiMessage::User(u)) => {
                out.push_str("USER:\n");
                match &u.content {
                    theway_llm_provider::UserContent::Text(s) => out.push_str(s),
                    theway_llm_provider::UserContent::Blocks(blocks) => {
                        for b in blocks {
                            match b {
                                theway_llm_provider::UserContentBlock::Text(t) => {
                                    out.push_str(&t.text)
                                }
                                theway_llm_provider::UserContentBlock::Image(_) => {
                                    out.push_str("<image>")
                                }
                            }
                        }
                    }
                }
                out.push_str("\n\n");
            }
            AgentMessage::Llm(PiMessage::Assistant(a)) => {
                out.push_str("ASSISTANT:\n");
                for b in &a.content {
                    match b {
                        theway_llm_provider::ContentBlock::Text(t) => out.push_str(&t.text),
                        theway_llm_provider::ContentBlock::Thinking(t) => {
                            out.push_str("<thinking>");
                            out.push_str(&t.thinking);
                            out.push_str("</thinking>");
                        }
                        theway_llm_provider::ContentBlock::Image(_) => out.push_str("<image>"),
                        theway_llm_provider::ContentBlock::ToolCall(tc) => {
                            out.push_str(&format!(
                                "<tool_call name=\"{}\">{}</tool_call>",
                                tc.name,
                                serde_json::Value::Object(tc.arguments.clone())
                            ));
                        }
                    }
                }
                out.push_str("\n\n");
            }
            AgentMessage::Llm(PiMessage::ToolResult(tr)) => {
                out.push_str(&format!("TOOL_RESULT[{}]:\n", tr.tool_name));
                for b in &tr.content {
                    if let theway_llm_provider::UserContentBlock::Text(t) = b {
                        out.push_str(&t.text);
                    }
                }
                out.push_str("\n\n");
            }
            AgentMessage::Custom(c) => {
                out.push_str(&format!("{}:\n{}\n\n", c.role.to_uppercase(), c.payload));
            }
        }
    }
    out
}

/// Prompt framing slack (message wrappers, the omission-note message, provider envelope).
const SUMMARY_PROMPT_FRAMING_TOKENS: u64 = 512;
/// Floor for the overflow-retry budget halving in [`compact`].
const MIN_SUMMARY_PROMPT_BUDGET_TOKENS: u64 = 1_024;
/// Maximum provider-overflow retries before compaction gives up.
const MAX_SUMMARY_OVERFLOW_RETRIES: u32 = 3;

/// Output cap sent as `max_tokens` on the summarizer call. Providers fall back to
/// `model.max_tokens` when unset, and `input + max_tokens > context_window` is a hard 400 on
/// Anthropic — so the summarizer must always send an explicit, bounded value.
fn summary_output_tokens(model: &Model, settings: &CompactionSettings) -> u32 {
    let reserve = if settings.reserve_tokens > 0 {
        settings.reserve_tokens
    } else {
        DEFAULT_COMPACTION_SETTINGS.reserve_tokens
    };
    let mut output = if model.max_tokens > 0 {
        model.max_tokens.min(reserve)
    } else {
        reserve
    };
    if model.context_window > 0 {
        output = output.min(model.context_window / 4).max(1);
    }
    output
}

fn summarization_prompt_budget(model: &Model, settings: &CompactionSettings) -> u64 {
    if model.context_window == 0 {
        return DEFAULT_SUMMARY_PROMPT_TOKEN_BUDGET;
    }
    let window = model.context_window as u64;
    let output = summary_output_tokens(model, settings) as u64;
    // Keep 20% slack below (window - output): the char-class token estimate can undercount on
    // code-heavy or mixed-script content, and Anthropic rejects input + max_tokens > window.
    window.saturating_sub(output).saturating_mul(4) / 5
}

fn summary_prompt_overhead_tokens(custom_instructions: Option<&str>) -> u64 {
    SUMMARY_PROMPT_FRAMING_TOKENS
        + estimate_text_tokens(SUMMARIZATION_SYSTEM_PROMPT)
        + custom_instructions
            .map(estimate_text_tokens)
            .unwrap_or_default()
}

fn summarize_prompt_estimate_tokens(
    messages: &[AgentMessage],
    custom_instructions: Option<&str>,
) -> u64 {
    let conversation: u64 = messages.iter().map(estimate_tokens).sum();
    summary_prompt_overhead_tokens(custom_instructions) + conversation
}

fn trim_messages_for_summary_budget(
    messages: &[AgentMessage],
    budget_tokens: u64,
    custom_instructions: Option<&str>,
) -> Vec<AgentMessage> {
    if summarize_prompt_estimate_tokens(messages, custom_instructions) <= budget_tokens {
        return messages.to_vec();
    }

    let mut kept = Vec::new();
    let mut total = summary_prompt_overhead_tokens(custom_instructions);
    for message in messages.iter().rev() {
        let message_tokens = estimate_tokens(message);
        if !kept.is_empty() && total + message_tokens > budget_tokens {
            break;
        }
        kept.push(message.clone());
        total = total.saturating_add(message_tokens);
        if total >= budget_tokens {
            break;
        }
    }
    kept.reverse();
    let omitted = messages.len().saturating_sub(kept.len());
    if omitted > 0 {
        kept.insert(
            0,
            AgentMessage::Llm(PiMessage::User(theway_llm_provider::UserMessage {
                role: theway_llm_provider::UserRole::User,
                content: theway_llm_provider::UserContent::Text(format!(
                    "[compaction note: omitted {omitted} older message(s) before summarization because the session exceeded the summarizer prompt budget]"
                )),
                timestamp: chrono::Utc::now().timestamp_millis(),
            })),
        );
    }
    kept
}

/// Byte index where the suffix of `s` last fits within `budget_tokens` by the char-class
/// estimate. Always lands on a char boundary.
fn suffix_start_for_token_budget(s: &str, budget_tokens: u64) -> usize {
    let mut ascii = 0u64;
    let mut non_ascii = 0u64;
    let mut start = s.len();
    for (idx, c) in s.char_indices().rev() {
        let (next_ascii, next_non_ascii) = if c.is_ascii() {
            (ascii + 1, non_ascii)
        } else {
            (ascii, non_ascii + 1)
        };
        if next_ascii.div_ceil(4) + next_non_ascii > budget_tokens {
            break;
        }
        ascii = next_ascii;
        non_ascii = next_non_ascii;
        start = idx;
    }
    start
}

fn serialize_conversation_for_summary_budget(
    messages: &[AgentMessage],
    budget_tokens: u64,
    custom_instructions: Option<&str>,
) -> String {
    let messages = trim_messages_for_summary_budget(messages, budget_tokens, custom_instructions);
    let conversation = serialize_conversation(&messages);
    let available_tokens =
        budget_tokens.saturating_sub(summary_prompt_overhead_tokens(custom_instructions));
    if estimate_text_tokens(&conversation) <= available_tokens {
        return conversation;
    }

    let note = "[compaction note: omitted older serialized content before summarization because the session exceeded the summarizer prompt budget]\n\n";
    let note_tokens = estimate_text_tokens(note);
    if available_tokens <= note_tokens {
        // The note is ASCII, so ~4 chars per token.
        return note
            .chars()
            .take(available_tokens.saturating_mul(4) as usize)
            .collect();
    }

    let start = suffix_start_for_token_budget(&conversation, available_tokens - note_tokens);
    format!("{note}{}", &conversation[start..])
}

#[derive(Clone)]
pub struct GenerateSummaryRequest {
    pub model: Model,
    pub messages: Vec<AgentMessage>,
    pub custom_instructions: Option<String>,
    pub prompt_budget_tokens: Option<u64>,
    /// Explicit `max_tokens` for the summarizer call. Providers fall back to `model.max_tokens`
    /// when `None`, which can push `input + max_tokens` past the context window.
    pub max_output_tokens: Option<u32>,
    /// Override stream function; falls back to `theway_llm_provider::stream_simple` when `None`.
    pub stream_fn: Option<StreamFn>,
}

#[derive(Clone, Debug)]
pub struct GenerateSummaryOutput {
    pub summary: String,
    pub usage: Usage,
}

/// Call the LLM to produce a single text summary of the supplied messages.
pub async fn generate_summary(
    request: GenerateSummaryRequest,
    cancel: CancellationToken,
) -> Result<GenerateSummaryOutput, SummarizeError> {
    let mut prompt = SUMMARIZATION_SYSTEM_PROMPT.to_string();
    if let Some(extra) = request.custom_instructions.as_deref() {
        prompt.push_str("\n\n");
        prompt.push_str(extra);
    }

    let convo = if let Some(budget) = request.prompt_budget_tokens {
        serialize_conversation_for_summary_budget(
            &request.messages,
            budget,
            request.custom_instructions.as_deref(),
        )
    } else {
        serialize_conversation(&request.messages)
    };
    let user = theway_llm_provider::UserMessage {
        role: theway_llm_provider::UserRole::User,
        content: theway_llm_provider::UserContent::Text(convo),
        timestamp: chrono::Utc::now().timestamp_millis(),
    };
    let context = PiContext {
        system_prompt: Some(prompt),
        messages: vec![theway_llm_provider::Message::User(user)],
        tools: None,
    };
    let stream_fn = request.stream_fn.unwrap_or_else(default_stream_fn);
    let mut options = SimpleStreamOptions::default();
    options.base.abort = Some(cancel.clone());
    options.base.max_tokens = request.max_output_tokens;

    let mut stream = stream_fn(&request.model, &context, Some(&options));
    let mut last: Option<AssistantMessage> = None;
    while let Some(ev) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(SummarizeError::Aborted);
        }
        match ev {
            AssistantMessageEvent::Done { message, .. } => last = Some(message),
            AssistantMessageEvent::Error { error, .. } => {
                let window = (request.model.context_window > 0)
                    .then_some(request.model.context_window as u64);
                let overflowed = theway_llm_provider::is_context_overflow(&error, window);
                let message = error
                    .error_message
                    .unwrap_or_else(|| "summarization failed".into());
                return Err(if overflowed {
                    SummarizeError::ContextOverflow(message)
                } else {
                    SummarizeError::Provider(message)
                });
            }
            _ => {}
        }
    }
    let msg = last.ok_or(SummarizeError::Empty)?;
    let summary = msg
        .content
        .iter()
        .filter_map(|b| match b {
            theway_llm_provider::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    Ok(GenerateSummaryOutput {
        summary,
        usage: msg.usage,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SummarizeError {
    #[error("aborted")]
    Aborted,
    #[error("provider error: {0}")]
    Provider(String),
    #[error("summarizer prompt overflowed the model context window: {0}")]
    ContextOverflow(String),
    #[error("summarizer produced no message")]
    Empty,
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// prepare_compaction + compact
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct CompactionPreparation {
    pub cut: CutPointResult,
    /// Messages that will be summarized (i.e., the prefix that compaction folds).
    pub entries_to_summarize: Vec<SessionTreeEntry>,
    /// Sum of estimated tokens for the prefix being summarized.
    pub tokens_before: u64,
}

pub fn prepare_compaction(
    entries: &[SessionTreeEntry],
    settings: &CompactionSettings,
) -> CompactionPreparation {
    let cut = find_cut_point(entries, settings);
    let entries_to_summarize = entries[..cut.cut_index].to_vec();
    let tokens_before = entries_to_summarize
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Message { message, .. } => Some(estimate_tokens(message)),
            _ => None,
        })
        .sum();
    CompactionPreparation {
        cut,
        entries_to_summarize,
        tokens_before,
    }
}

#[derive(Clone, Debug)]
pub struct CompactionResult {
    pub summary: String,
    pub first_kept_entry_id: Option<String>,
    pub tokens_before: u64,
    pub usage: Usage,
}

/// Top-level compaction entry point. Picks a cut point via the algorithm, summarizes the
/// prefix via the algorithm's summarize hook, returns the summary plus metadata for the
/// harness to record on the session.
pub async fn compact(
    algorithm: &dyn CompactAlgorithm,
    model: Model,
    entries: &[SessionTreeEntry],
    settings: &CompactionSettings,
    custom_instructions: Option<String>,
    stream_fn: Option<StreamFn>,
    cancel: CancellationToken,
) -> Result<CompactionResult, SummarizeError> {
    let cut = algorithm.select_cut_point(entries, settings).await;
    let entries_to_summarize = &entries[..cut.cut_index];
    let tokens_before = entries_to_summarize
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Message { message, .. } => Some(estimate_tokens(message)),
            _ => None,
        })
        .sum();
    if entries_to_summarize.is_empty() {
        return Ok(CompactionResult {
            summary: String::new(),
            first_kept_entry_id: cut.first_kept_entry_id,
            tokens_before,
            usage: Usage::default(),
        });
    }
    // Project the entries into AgentMessage[] for the summarizer.
    let messages: Vec<AgentMessage> = entries_to_summarize
        .iter()
        .filter_map(|e| match e {
            SessionTreeEntry::Message { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    let request = SummarizeRequest {
        model: &model,
        messages: &messages,
        custom_instructions: custom_instructions.as_deref(),
        settings,
        stream_fn: stream_fn.as_ref(),
        cancel: &cancel,
    };
    let out = algorithm.summarize_prefix(&request).await?;
    Ok(CompactionResult {
        summary: out.summary,
        first_kept_entry_id: cut.first_kept_entry_id,
        tokens_before,
        usage: out.usage,
    })
}

/// LLM-backed summarize used by the builtin algorithm (and as the trait default). Runs the
/// overflow-retry budget loop: the prompt budget is a char-class estimate, so the provider
/// can still reject the call as a context overflow — halve the budget and retry instead of
/// failing the whole compaction.
pub async fn summarize_with_llm(
    request: &SummarizeRequest<'_>,
) -> Result<SummaryOutcome, SummarizeError> {
    let max_output_tokens = summary_output_tokens(request.model, request.settings);
    let mut budget = summarization_prompt_budget(request.model, request.settings);
    let mut attempts = 0u32;
    let out = loop {
        let result = generate_summary(
            GenerateSummaryRequest {
                model: request.model.clone(),
                messages: request.messages.to_vec(),
                custom_instructions: request.custom_instructions.map(str::to_string),
                prompt_budget_tokens: Some(budget),
                max_output_tokens: Some(max_output_tokens),
                stream_fn: request.stream_fn.cloned(),
            },
            request.cancel.clone(),
        )
        .await;
        match result {
            Ok(out) => break out,
            Err(SummarizeError::ContextOverflow(message)) => {
                attempts += 1;
                if attempts > MAX_SUMMARY_OVERFLOW_RETRIES
                    || budget <= MIN_SUMMARY_PROMPT_BUDGET_TOKENS
                {
                    return Err(SummarizeError::ContextOverflow(message));
                }
                budget = (budget / 2).max(MIN_SUMMARY_PROMPT_BUDGET_TOKENS);
            }
            Err(e) => return Err(e),
        }
    };
    Ok(SummaryOutcome {
        summary: out.summary,
        usage: out.usage,
    })
}

#[cfg(test)]
// Test files live in `tests/runtime/compaction/compaction/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("runtime/compaction/compaction");
