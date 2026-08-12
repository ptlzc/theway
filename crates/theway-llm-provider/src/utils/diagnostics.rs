//! Structured diagnostic emission. 1:1 stub of `packages/ai/src/utils/diagnostics.ts`.

use serde::{Deserialize, Serialize};

/// Provider/runtime diagnostic attached to an `AssistantMessage` when something noteworthy
/// happens (model fallback, deprecated option ignored, retry success, etc.).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantMessageDiagnostic {
    pub kind: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}
