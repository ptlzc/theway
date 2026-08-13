//! User-Agent construction. 1:1 port of `packages/ai/src/utils/headers.ts`.

pub fn user_agent() -> String {
    format!("theway-llm-provider-rs/{}", env!("CARGO_PKG_VERSION"))
}
