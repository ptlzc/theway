//! Context-overflow detection. 1:1 port of `packages/ai/src/utils/overflow.ts`.
//!
//! `is_context_overflow` recognises the "context too long" error shapes across providers, plus
//! two silent-overflow signals (usage exceeding the window; length-stop with zero output). The
//! agent harness uses this to trigger compaction.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::{AssistantMessage, StopReason};

/// Compiled overflow patterns. The TS list is reproduced verbatim (case-insensitive).
static OVERFLOW_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"prompt is too long",                    // Anthropic token overflow
        r"request_too_large",                     // Anthropic 413 byte-size
        r"input is too long for requested model", // Amazon Bedrock
        r"exceeds the context window",            // OpenAI (Completions & Responses)
        r"exceeds (?:the )?(?:model'?s )?maximum context length of [\d,]+ tokens?", // LiteLLM proxies
        r"input token count.*exceeds the maximum", // Google (Gemini)
        r"maximum prompt length is \d+",           // xAI (Grok)
        r"reduce the length of the messages",      // Groq
        r"maximum context length is \d+ tokens",   // OpenRouter
        r"input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)", // Together AI
        r"exceeds the limit of \d+",           // GitHub Copilot
        r"exceeds the available context size", // llama.cpp
        r"greater than the context length",    // LM Studio
        r"context window exceeds limit",       // MiniMax
        r"exceeded model token limit",         // Kimi For Coding
        r"too large for model with \d+ maximum context length", // Mistral
        r"model_context_window_exceeded",      // z.ai surfaced as text
        r"prompt too long; exceeded (?:max )?context length", // Ollama
        r"context[_ ]length[_ ]exceeded",      // generic
        r"too many tokens",                    // generic
        r"token limit exceeded",               // generic
        r"^4(?:00|13)\s*(?:status code)?\s*\(no body\)", // Cerebras 400/413 no body
    ]
    .iter()
    .map(|p| Regex::new(&format!("(?i){p}")).expect("valid overflow regex"))
    .collect()
});

/// Patterns that indicate a non-overflow error even if they also match an overflow pattern
/// (e.g. Bedrock throttling "Too many tokens, please wait").
static NON_OVERFLOW_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"^(Throttling error|Service unavailable):",
        r"rate limit",
        r"too many requests",
    ]
    .iter()
    .map(|p| Regex::new(&format!("(?i){p}")).expect("valid non-overflow regex"))
    .collect()
});

/// Report from [`is_context_overflow`]. The TS version returns a bare bool; we keep the richer
/// `ContextOverflow` shape declared earlier for callers that want the detail, plus a bool helper.
#[derive(Clone, Debug, Default)]
pub struct ContextOverflow {
    pub overflowed: bool,
    pub requested_tokens: Option<u64>,
    pub limit_tokens: Option<u64>,
}

/// Returns true if the assistant message represents a context overflow.
///
/// `context_window` enables silent-overflow detection (z.ai-style success with oversized usage,
/// and Xiaomi-style length-stop with zero output). Pass `None` to only check error patterns.
pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    // Case 1: error message patterns.
    if message.stop_reason == StopReason::Error {
        if let Some(err) = &message.error_message {
            let is_non_overflow = NON_OVERFLOW_PATTERNS.iter().any(|p| p.is_match(err));
            if !is_non_overflow && OVERFLOW_PATTERNS.iter().any(|p| p.is_match(err)) {
                return true;
            }
        }
    }

    // Case 2: silent overflow (z.ai) — success but usage exceeds the window.
    if let Some(cw) = context_window {
        if message.stop_reason == StopReason::Stop {
            let input = message.usage.input + message.usage.cache_read;
            if input > cw {
                return true;
            }
        }
        // Case 3: length-stop overflow (Xiaomi MiMo) — truncated input fills the window with no
        // room to generate (output == 0).
        if message.stop_reason == StopReason::Length && message.usage.output == 0 {
            let input = message.usage.input + message.usage.cache_read;
            if input as f64 >= cw as f64 * 0.99 {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn err_msg(text: &str) -> AssistantMessage {
        AssistantMessage {
            role: AssistantRole::Assistant,
            content: vec![],
            api: Api::from("anthropic-messages"),
            provider: Provider::from("anthropic"),
            model: "m".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Error,
            error_message: Some(text.into()),
            timestamp: 0,
        }
    }

    #[test]
    fn detects_anthropic_overflow() {
        assert!(is_context_overflow(
            &err_msg("prompt is too long: 213462 tokens > 200000 maximum"),
            None
        ));
    }

    #[test]
    fn detects_openai_and_gemini() {
        assert!(is_context_overflow(
            &err_msg("Your input exceeds the context window of this model"),
            None
        ));
        assert!(is_context_overflow(
            &err_msg(
                "The input token count (1196265) exceeds the maximum number of tokens allowed"
            ),
            None
        ));
    }

    #[test]
    fn excludes_rate_limit() {
        // `formatBedrockError` rewrites the raw "ThrottlingException" into a "Throttling error:"
        // prefix, which the NON_OVERFLOW pattern matches even though the body says "too many tokens".
        assert!(!is_context_overflow(
            &err_msg("Throttling error: Too many tokens, please wait before trying again."),
            None
        ));
        assert!(!is_context_overflow(&err_msg("rate limit exceeded"), None));
    }

    #[test]
    fn silent_overflow_via_usage() {
        let mut m = err_msg("");
        m.stop_reason = StopReason::Stop;
        m.error_message = None;
        m.usage.input = 250_000;
        assert!(is_context_overflow(&m, Some(200_000)));
        assert!(!is_context_overflow(&m, Some(300_000)));
    }

    #[test]
    fn length_stop_zero_output() {
        let mut m = err_msg("");
        m.stop_reason = StopReason::Length;
        m.error_message = None;
        m.usage.input = 199_000;
        m.usage.output = 0;
        assert!(is_context_overflow(&m, Some(200_000)));
    }
}
