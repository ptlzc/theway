//! Header merging + User-Agent construction. 1:1 port of `packages/ai/src/utils/headers.ts`.

use std::collections::HashMap;

pub fn user_agent() -> String {
    format!("theway-llm-provider-rs/{}", env!("CARGO_PKG_VERSION"))
}

/// Merge two header maps. Right side wins on collision (mirrors TS `{ ...a, ...b }`).
pub fn merge_headers(
    base: &HashMap<String, String>,
    overrides: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let mut out = base.clone();
    if let Some(over) = overrides {
        for (k, v) in over {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}
