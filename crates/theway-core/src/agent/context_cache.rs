//! Client-side prefix cache hit estimation.
//!
//! The provider reports `cache_read_tokens` for real prompt-cache reads, but
//! there is no provider-agnostic signal for how much of the *final* context
//! prefix actually overlapped with the previous request. This module implements
//! a lightweight, tokenizer-free estimate:
//!
//! 1. Serialize the final provider `Context` into a canonical byte sequence.
//! 2. Split the bytes into fixed-size chunks and hash each chunk.
//! 3. Compare the current chunk list with the previous request's list from
//!    index 0 (longest common prefix).
//! 4. Convert overlapping bytes to tokens using the provider-reported total
//!    input token count as the byte-to-token calibration.
//!
//! The estimate is intentionally approximate and only intended to explain cache
//! trends, not to replace provider-reported cache accounting.

use std::collections::HashMap;

use serde_json::{Value, json};
use theway_llm_provider::{
    ContentBlock, Context as PiContext, Message, UserContent, UserContentBlock,
};

/// Chunk size for the prefix-overlap comparison.
pub const CONTEXT_CHUNK_SIZE: usize = 256;

/// Result of comparing the current context against the previous baseline.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrefixHitEstimate {
    /// Number of bytes in the longest common prefix between the current and
    /// previous canonical context.
    pub overlap_bytes: usize,
    /// Total canonical bytes of the current context.
    pub total_bytes: usize,
}

/// Final prefix-hit metrics after provider usage is available.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrefixHitResult {
    /// Estimated number of input tokens served from the context prefix.
    pub prefix_hit_tokens: u64,
    /// `prefix_hit_tokens / total_input_tokens`; `None` when there is no
    /// provider-reported total input to calibrate against.
    pub prefix_cache_hit_rate: Option<f64>,
}

#[derive(Clone, Debug, Default)]
struct ContextCacheEntry {
    chunk_hashes: Vec<u64>,
    bytes: Vec<u8>,
}

/// Per-session, per-model prefix overlap tracker.
///
/// The baseline is keyed by `(session_id, provider, model)`. Changing the
/// provider or model clears that key's previous baseline so the first request
/// after a switch reports a low (zero) prefix hit rate.
#[derive(Clone, Debug, Default)]
pub struct ContextCacheTracker {
    entries: HashMap<String, ContextCacheEntry>,
    /// Last key used per session, used to reset a key when the provider/model
    /// changes away and back.
    last_keys: HashMap<String, String>,
}

impl ContextCacheTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compare `context` against the stored baseline for the active key, then
    /// store this context as the new baseline.
    ///
    /// Call this immediately before sending the request, after all context
    /// transforms have been applied.
    pub fn estimate(
        &mut self,
        session_id: Option<&str>,
        provider: &str,
        model: &str,
        context: &PiContext,
    ) -> PrefixHitEstimate {
        let session = session_id.unwrap_or("");
        let key = format!("{session}\0{provider}\0{model}");

        if let Some(previous_key) = self.last_keys.get(session) {
            if previous_key != &key {
                // Model/provider switch: start the new key's baseline from
                // scratch even if it was seen earlier in the session.
                self.entries.remove(&key);
            }
        }
        self.last_keys.insert(session.to_string(), key.clone());

        let bytes = canonical_context_bytes(context);
        let total_bytes = bytes.len();
        let chunk_hashes = chunk_hashes(&bytes);
        let overlap_bytes = self
            .entries
            .get(&key)
            .map(|entry| longest_common_prefix_bytes(entry, &bytes))
            .unwrap_or(0);

        self.entries.insert(
            key,
            ContextCacheEntry {
                chunk_hashes,
                bytes,
            },
        );

        PrefixHitEstimate {
            overlap_bytes,
            total_bytes,
        }
    }

    /// Compute the token-level prefix estimate once the provider reports total
    /// input tokens for the request.
    pub fn finalize(
        &self,
        estimate: &PrefixHitEstimate,
        total_input_tokens: u64,
    ) -> PrefixHitResult {
        if estimate.total_bytes == 0 || total_input_tokens == 0 {
            return PrefixHitResult {
                prefix_hit_tokens: 0,
                prefix_cache_hit_rate: None,
            };
        }

        let prefix_hit_tokens = ((estimate.overlap_bytes as u128) * (total_input_tokens as u128)
            / (estimate.total_bytes as u128)) as u64;
        let rate = prefix_hit_tokens as f64 / total_input_tokens as f64;
        PrefixHitResult {
            prefix_hit_tokens,
            prefix_cache_hit_rate: Some(rate),
        }
    }

    /// Drop all baselines for a session (e.g. session reset/clear).
    pub fn clear_session(&mut self, session_id: Option<&str>) {
        let session = session_id.unwrap_or("");
        self.last_keys.remove(session);
        self.entries
            .retain(|key, _| !key.starts_with(&format!("{session}\0")));
    }
}

/// Canonical, deterministic byte representation of the final provider context.
///
/// Only fields that affect the provider request body are included. Transient
/// bookkeeping such as usage counters, costs, response ids, diagnostics, and
/// timestamps is excluded so unchanged conversation prefixes produce stable
/// hashes across turns. `serde_json::Value` object keys are sorted recursively
/// so the same logical context hashes identically regardless of map insertion
/// order.
pub fn canonical_context_bytes(context: &PiContext) -> Vec<u8> {
    let messages = context
        .messages
        .iter()
        .map(canonical_message)
        .collect::<Vec<_>>();
    let tools = context.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect::<Vec<_>>()
    });
    let mut value = json!({
        "system_prompt": context.system_prompt,
        "messages": messages,
        "tools": tools,
    });
    canonicalize_structural(&mut value);
    serde_json::to_vec(&value).unwrap_or_default()
}

fn canonical_message(message: &Message) -> Value {
    match message {
        Message::User(message) => json!({
            "role": "user",
            "content": canonical_user_content(&message.content),
        }),
        Message::Assistant(message) => json!({
            "role": "assistant",
            "content": message
                .content
                .iter()
                .map(canonical_content_block)
                .collect::<Vec<_>>(),
        }),
        Message::ToolResult(message) => json!({
            "role": "tool_result",
            "tool_call_id": message.tool_call_id,
            "tool_name": message.tool_name,
            "content": message
                .content
                .iter()
                .map(canonical_user_content_block)
                .collect::<Vec<_>>(),
            "is_error": message.is_error,
        }),
    }
}

fn canonical_user_content(content: &UserContent) -> Value {
    match content {
        UserContent::Text(text) => Value::String(text.clone()),
        UserContent::Blocks(blocks) => {
            Value::Array(blocks.iter().map(canonical_user_content_block).collect())
        }
    }
}

fn canonical_content_block(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(text) => json!({
            "type": "text",
            "text": text.text,
            "text_signature": text.text_signature,
        }),
        ContentBlock::Thinking(thinking) => json!({
            "type": "thinking",
            "thinking": thinking.thinking,
            "thinking_signature": thinking.thinking_signature,
            "redacted": thinking.redacted,
        }),
        ContentBlock::Image(image) => json!({
            "type": "image",
            "mime_type": image.mime_type,
            "data": image.data,
        }),
        ContentBlock::ToolCall(call) => json!({
            "type": "tool_call",
            "id": call.id,
            "name": call.name,
            "arguments": call.arguments,
            "thought_signature": call.thought_signature,
        }),
    }
}

fn canonical_user_content_block(block: &UserContentBlock) -> Value {
    match block {
        UserContentBlock::Text(text) => json!({
            "type": "text",
            "text": text.text,
            "text_signature": text.text_signature,
        }),
        UserContentBlock::Image(image) => json!({
            "type": "image",
            "mime_type": image.mime_type,
            "data": image.data,
        }),
    }
}

/// Canonicalize the structural wrapper without reordering top-level fields.
/// The logical provider context order (system prompt, messages, tools) is
/// significant for prefix matching, so only free-form JSON payloads (tool
/// parameters, tool-call arguments) have their object keys sorted.
fn canonicalize_structural(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "parameters" || key == "arguments" {
                    canonicalize_freeform(child);
                } else {
                    canonicalize_structural(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                canonicalize_structural(item);
            }
        }
        _ => {}
    }
}

/// Recursively sort object keys in a free-form JSON value.
fn canonicalize_freeform(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for child in map.values_mut() {
                canonicalize_freeform(child);
            }
            map.sort_keys();
        }
        Value::Array(items) => {
            for item in items {
                canonicalize_freeform(item);
            }
        }
        _ => {}
    }
}

fn chunk_hashes(bytes: &[u8]) -> Vec<u64> {
    bytes.chunks(CONTEXT_CHUNK_SIZE).map(fnv1a).collect()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn longest_common_prefix_bytes(previous: &ContextCacheEntry, current: &[u8]) -> usize {
    let current_hashes = chunk_hashes(current);
    let mut matched_chunks = 0usize;
    for (prev_hash, curr_hash) in previous.chunk_hashes.iter().zip(&current_hashes) {
        if prev_hash == curr_hash {
            matched_chunks += 1;
        } else {
            break;
        }
    }

    let offset = matched_chunks
        .saturating_mul(CONTEXT_CHUNK_SIZE)
        .min(previous.bytes.len())
        .min(current.len());

    if matched_chunks < previous.chunk_hashes.len().min(current_hashes.len()) {
        // The next chunk differs; count the common byte prefix inside it.
        let start = offset;
        let prev_slice =
            &previous.bytes[start..previous.bytes.len().min(start + CONTEXT_CHUNK_SIZE)];
        let curr_slice = &current[start..current.len().min(start + CONTEXT_CHUNK_SIZE)];
        return offset.saturating_add(
            prev_slice
                .iter()
                .zip(curr_slice)
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }

    // All common chunks matched; include the tail of a partial final chunk.
    offset.saturating_add(
        previous.bytes[offset..]
            .iter()
            .zip(&current[offset..])
            .take_while(|(a, b)| a == b)
            .count(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_llm_provider::{
        AssistantMessage, AssistantRole, ContentBlock, Message, StopReason, Tool,
        ToolResultMessage, ToolResultRole, Usage, UserContent, UserContentBlock, UserMessage,
        UserRole,
    };

    fn context_with(extra: Option<&str>) -> PiContext {
        let mut messages = vec![Message::User(UserMessage {
            role: UserRole::User,
            content: UserContent::Text("hello".into()),
            timestamp: 1,
        })];
        if let Some(text) = extra {
            messages.push(Message::Assistant(AssistantMessage {
                role: AssistantRole::Assistant,
                content: vec![ContentBlock::Text(theway_llm_provider::TextContent {
                    text: text.into(),
                    text_signature: None,
                })],
                api: theway_llm_provider::Api::from("faux"),
                provider: theway_llm_provider::Provider::from("faux"),
                model: "m".into(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 2,
            }));
        }
        PiContext {
            system_prompt: Some("system".into()),
            messages,
            tools: Some(vec![Tool {
                name: "t".into(),
                description: "d".into(),
                parameters: serde_json::json!({ "type": "object" }),
            }]),
        }
    }

    #[test]
    fn append_only_context_has_high_prefix_overlap() {
        let mut tracker = ContextCacheTracker::new();
        let first = context_with(None);
        let estimate = tracker.estimate(Some("s1"), "p", "m", &first);
        assert_eq!(estimate.overlap_bytes, 0);
        assert!(estimate.total_bytes > 0);

        let second = context_with(Some("next turn"));
        let estimate = tracker.estimate(Some("s1"), "p", "m", &second);
        assert!(estimate.overlap_bytes > 0);
        assert!(estimate.overlap_bytes < estimate.total_bytes);

        let result = tracker.finalize(&estimate, 100);
        assert!(result.prefix_hit_tokens > 0);
        assert!(result.prefix_cache_hit_rate.unwrap() > 0.0);
    }

    #[test]
    fn mid_insertion_lowers_prefix_overlap() {
        let mut tracker = ContextCacheTracker::new();
        let base = context_with(Some("same tail"));
        tracker.estimate(Some("s1"), "p", "m", &base);

        // Changing the system prompt (the very start) drops the prefix to a
        // tiny JSON-envelope remainder instead of a large content overlap.
        let mut changed = base.clone();
        changed.system_prompt = Some("different system".into());
        let estimate = tracker.estimate(Some("s1"), "p", "m", &changed);
        assert!(
            estimate.overlap_bytes < 64,
            "overlap: {}",
            estimate.overlap_bytes
        );
    }

    #[test]
    fn model_switch_resets_baseline() {
        let mut tracker = ContextCacheTracker::new();
        let context = context_with(None);
        tracker.estimate(Some("s1"), "p", "model-a", &context);
        let estimate = tracker.estimate(Some("s1"), "p", "model-b", &context);
        assert_eq!(estimate.overlap_bytes, 0);

        // Switching back to model-a also starts fresh.
        let estimate = tracker.estimate(Some("s1"), "p", "model-a", &context);
        assert_eq!(estimate.overlap_bytes, 0);
    }

    #[test]
    fn missing_total_input_returns_unknown_rate() {
        let tracker = ContextCacheTracker::new();
        let estimate = PrefixHitEstimate {
            overlap_bytes: 10,
            total_bytes: 100,
        };
        let result = tracker.finalize(&estimate, 0);
        assert_eq!(result.prefix_hit_tokens, 0);
        assert_eq!(result.prefix_cache_hit_rate, None);
    }

    #[test]
    fn compaction_drop_of_suffix_lowers_prefix_overlap() {
        let mut tracker = ContextCacheTracker::new();
        tracker.estimate(Some("s1"), "p", "m", &context_with(Some("long tail")));
        let compacted = context_with(None);
        let estimate = tracker.estimate(Some("s1"), "p", "m", &compacted);
        assert!(
            estimate.overlap_bytes < estimate.total_bytes / 2,
            "compaction should break prefix: {} / {}",
            estimate.overlap_bytes,
            estimate.total_bytes
        );
    }

    #[test]
    fn virtualization_keeps_earlier_prefix_stable() {
        let mut tracker = ContextCacheTracker::new();
        let full = context_with_tool_result("actual large tool output line");
        tracker.estimate(Some("s1"), "p", "m", &full);
        let virtualized =
            context_with_tool_result("[tool_result bash call_1: 100 bytes / 10 lines; tail: ...]");
        let estimate = tracker.estimate(Some("s1"), "p", "m", &virtualized);
        assert!(
            estimate.overlap_bytes > estimate.total_bytes / 2,
            "virtualization should keep the earlier prefix: {} / {}",
            estimate.overlap_bytes,
            estimate.total_bytes
        );
    }

    fn context_with_tool_result(result_text: &str) -> PiContext {
        let mut context = context_with(Some("assistant text"));
        context
            .messages
            .push(Message::ToolResult(ToolResultMessage {
                role: ToolResultRole::ToolResult,
                tool_call_id: "call_1".into(),
                tool_name: "bash".into(),
                content: vec![UserContentBlock::text(result_text)],
                details: None,
                is_error: false,
                timestamp: 3,
            }));
        context
    }

    #[test]
    fn canonicalization_sorts_freeform_object_keys() {
        let mut a = serde_json::json!({ "z": 1, "a": { "y": 2, "b": 3 } });
        let mut b = serde_json::json!({ "a": { "b": 3, "y": 2 }, "z": 1 });
        canonicalize_freeform(&mut a);
        canonicalize_freeform(&mut b);
        assert_eq!(a, b);
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
    }
}
