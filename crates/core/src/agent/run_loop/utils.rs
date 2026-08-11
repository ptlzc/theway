//! Free helper functions shared by the agent loop: event emission, context snapshots,
//! turn updates, and control-plane prompt payload helpers.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::AgentInner;
use crate::types::*;

pub(super) fn apply_turn_update(inner: &Arc<AgentInner>, update: AgentLoopTurnUpdate) {
    let mut state = inner.state.lock();
    if let Some(ctx) = update.context {
        state.messages = ctx.messages;
        state.system_prompt = ctx.system_prompt;
        state.tools = ctx.tools;
    }
    if let Some(model) = update.model {
        state.model = Some(model);
    }
    if let Some(level) = update.thinking_level {
        state.thinking_level = Some(level);
    }
}

pub(super) fn snapshot_context(inner: &Arc<AgentInner>) -> AgentContext {
    let g = inner.state.lock();
    AgentContext {
        system_prompt: g.system_prompt.clone(),
        messages: g.messages.clone(),
        tools: g.tools.clone(),
    }
}

pub(super) async fn emit(inner: &Arc<AgentInner>, event: LoopEvent, cancel: &CancellationToken) {
    let listeners = inner.listeners.lock().clone();
    for listener in listeners {
        let token = cancel.clone();
        listener(event.clone(), token).await;
    }
}

pub(super) async fn finalize(inner: &Arc<AgentInner>, cancel: CancellationToken) {
    let messages = inner.state.lock().messages.clone();
    emit(inner, LoopEvent::RunEnded { messages }, &cancel).await;
    inner.state.lock().is_streaming = false;
    *inner.active_cancel.lock() = None;
    *inner.turn_cancel.lock() = None;
    inner.idle.notify_waiters();
}

/// Canonical-JSON SHA-256 of the prepared tool args. Binds a control-plane prompt
/// approval to the exact invocation (issue #110 design v0.2 §1 Decision binding).
///
/// Canonicalization rules: object keys sorted lexicographically, no extra whitespace,
/// stable encoding of every numeric / string value. We use `serde_json` with the keys
/// pre-sorted by walking the tree — `serde_json::to_string` does NOT sort object keys by
/// default, so we re-serialize through a `BTreeMap` projection for objects.
pub(super) fn compute_args_hash(args: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let canonical = canonicalize(args);
    let bytes = serde_json::to_vec(&canonical)
        .unwrap_or_else(|_| b"<args canonicalization failed>".to_vec());
    let digest = Sha256::digest(&bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
    use std::collections::BTreeMap;
    match value {
        serde_json::Value::Object(map) => {
            // BTreeMap iterates in key order — produces sorted JSON object on re-serialize.
            let sorted: BTreeMap<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect();
            serde_json::to_value(sorted).unwrap_or(serde_json::Value::Null)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// Default **redaction-safe by construction** prompt payload synthesized from the
/// classifier outcome. Per @Provider-Auth-Lead + @CLI-TUI-Dev-Lead on PR #135 review:
/// the runtime must NEVER emit raw prepared args in the default payload — control-plane
/// tools often carry URLs with tokens, secret-bearing values, or large blobs whose raw
/// rendering would defeat the entire prompt-card audit story.
///
/// Default payload contains only:
/// - `tool_name` — display label only.
/// - `args_keys` — the top-level argument key names (sorted, ≤ 32 keys, each ≤ 64 chars).
///   Reveals what *categories* of input the tool received without revealing the values.
/// - `args_hash` — the same SHA-256 the runtime uses for anti-replay binding. Lets the
///   prompt UI render a stable per-call identifier (e.g. for "this is the same write
///   you approved 10 seconds ago" UX).
///
/// Tools that want a *richer* card (preview of the install source URL with token
/// stripped, diff of a config edit, etc.) override via `before_tool_call` returning
/// their own `BeforeToolCallResult.prompt` with a hand-redacted `payload`. Runtime
/// re-binds the authoritative `tool_call_id` / `tool_name` / `args_hash` fields after
/// the override, so the hook cannot accidentally weaken the binding.
pub(super) fn default_prompt_payload(
    tool_name: &str,
    args: &serde_json::Value,
) -> serde_json::Value {
    const MAX_KEYS: usize = 32;
    const MAX_KEY_LEN: usize = 64;
    let keys: Vec<String> = match args {
        serde_json::Value::Object(map) => {
            let mut ks: Vec<String> = map
                .keys()
                .take(MAX_KEYS)
                .map(|k| {
                    if k.chars().count() <= MAX_KEY_LEN {
                        k.clone()
                    } else {
                        let mut t: String = k.chars().take(MAX_KEY_LEN).collect();
                        t.push('…');
                        t
                    }
                })
                .collect();
            ks.sort();
            ks
        }
        _ => Vec::new(),
    };
    serde_json::json!({
        "tool_name": tool_name,
        "args_keys": keys,
        "args_hash": compute_args_hash(args),
    })
}
