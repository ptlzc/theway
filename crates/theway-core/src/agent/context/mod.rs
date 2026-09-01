//! LLM context assembly, loading, and unloading.
//!
//! This module owns the deterministic transforms that turn persisted session
//! entries into the `AgentMessage` list sent to the model, plus the collapse
//! context injection that lets a new session inherit the old session's compact
//! summary and lineage.

#[cfg(feature = "harness")]
pub mod assembly;
#[cfg(feature = "harness")]
pub mod collapse;
pub mod transform;

#[cfg(feature = "harness")]
pub use assembly::build_session_context;
pub use transform::{
    TOOL_RESULT_FRONT_PREVIEW_CHARS, TOOL_RESULT_TAIL_PREVIEW_LINE_CHARS,
    TOOL_RESULT_TAIL_PREVIEW_LINES, TOOL_RESULT_VIRTUALIZATION_MAX_CHARS,
    TOOL_RESULT_VIRTUALIZATION_MAX_CHARS_ENV, virtualize_tool_results,
};

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/context");
