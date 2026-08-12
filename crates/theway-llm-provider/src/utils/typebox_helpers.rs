//! Convenience constructors for tool parameter schemas. TS uses typebox; in Rust we hand-roll
//! `serde_json::Value` shapes (per Q3:A — JSON Schema literals are the contract).
//! 1:1 stub of `packages/ai/src/utils/typebox-helpers.ts`.

use serde_json::{Value, json};

/// JSON Schema for a required string field.
pub fn string(description: impl Into<String>) -> Value {
    json!({ "type": "string", "description": description.into() })
}

/// JSON Schema for a required boolean.
pub fn boolean(description: impl Into<String>) -> Value {
    json!({ "type": "boolean", "description": description.into() })
}

/// JSON Schema for a required number.
pub fn number(description: impl Into<String>) -> Value {
    json!({ "type": "number", "description": description.into() })
}

/// JSON Schema for an object with the given properties and required-list.
pub fn object(props: Value, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}
