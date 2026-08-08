//! Static model catalog. 1:1 port of `packages/ai/src/models.generated.ts`.
//!
//! **Do not edit by hand.** Source of truth is the TS file; we extract it to
//! `models.generated.json` (via `node --experimental-strip-types`) and `include_str!` the JSON
//! payload here so updates flow in by rerunning the extraction.
//!
//! Per Q4:A we picked `build.rs` codegen — but a runtime-parse-on-first-use approach is
//! cheaper to maintain (no build script to bitrot) and the parse cost for ~500 models is
//! sub-millisecond. We can swap in build.rs later if startup latency ever matters.

use std::collections::HashMap;

use once_cell::sync::Lazy;
use serde::Deserialize;

use crate::types::Model;

const CATALOG_JSON: &str = include_str!("models.generated.json");

#[derive(Deserialize)]
struct RawModel {
    id: String,
    name: String,
    api: String,
    provider: String,
    #[serde(rename = "baseUrl")]
    base_url: String,
    reasoning: bool,
    #[serde(default)]
    input: Vec<String>,
    cost: RawCost,
    #[serde(rename = "contextWindow")]
    context_window: u32,
    #[serde(rename = "maxTokens")]
    max_tokens: u32,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default, rename = "thinkingLevelMap")]
    thinking_level_map: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    compat: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawCost {
    input: f64,
    output: f64,
    #[serde(rename = "cacheRead")]
    cache_read: f64,
    #[serde(rename = "cacheWrite")]
    cache_write: f64,
}

pub static BUILTIN_MODELS: Lazy<Vec<Model>> = Lazy::new(|| {
    // Top level: { provider: { model_id: Model, ... }, ... }
    let nested: HashMap<String, HashMap<String, RawModel>> =
        serde_json::from_str(CATALOG_JSON).expect("models.generated.json is malformed");

    let mut out = Vec::with_capacity(1024);
    for (_provider, models) in nested {
        for (_id, raw) in models {
            out.push(convert(raw));
        }
    }
    out
});

fn convert(r: RawModel) -> Model {
    use crate::types::{Api, InputModality, ModelCost, ModelThinkingLevel, Provider};

    let input = r
        .input
        .into_iter()
        .filter_map(|s| match s.as_str() {
            "text" => Some(InputModality::Text),
            "image" => Some(InputModality::Image),
            _ => None,
        })
        .collect();

    let thinking_level_map = r.thinking_level_map.map(|m| {
        m.into_iter()
            .filter_map(|(k, v)| {
                let key = match k.as_str() {
                    "off" => ModelThinkingLevel::Off,
                    "minimal" => ModelThinkingLevel::Minimal,
                    "low" => ModelThinkingLevel::Low,
                    "medium" => ModelThinkingLevel::Medium,
                    "high" => ModelThinkingLevel::High,
                    "xhigh" => ModelThinkingLevel::Xhigh,
                    _ => return None,
                };
                Some((key, v))
            })
            .collect()
    });

    Model {
        id: r.id,
        name: r.name,
        api: Api::from(r.api),
        provider: Provider::from(r.provider),
        base_url: r.base_url,
        reasoning: r.reasoning,
        thinking_level_map,
        input,
        cost: ModelCost {
            input: r.cost.input,
            output: r.cost.output,
            cache_read: r.cost.cache_read,
            cache_write: r.cost.cache_write,
        },
        context_window: r.context_window,
        max_tokens: r.max_tokens,
        headers: r.headers,
        compat: r.compat,
    }
}
