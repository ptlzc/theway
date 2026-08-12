//! TODO: 1:1 port of `packages/ai/src/providers/github-copilot-headers.ts`. GitHub Copilot
//! injects extra headers (`copilot-integration-id`, `editor-version`, etc.) into the underlying
//! Anthropic-messages / OpenAI-responses requests.

use std::collections::HashMap;

pub fn copilot_headers() -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert("copilot-integration-id".into(), "vscode-chat".into());
    h.insert(
        "editor-version".into(),
        "theway-llm-provider-rs/0.75".into(),
    );
    h
}
