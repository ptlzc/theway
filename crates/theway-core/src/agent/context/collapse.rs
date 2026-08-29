//! Collapse context: parsing and injecting the old session's compact summary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::session::session::SessionTreeEntry;

/// Custom entry type written into a collapse child session.
pub const COMPACT_CONTEXT_CUSTOM_TYPE: &str = "compact_context";

/// Structured view of a `compact_context` custom entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactContext {
    pub source_session_id: String,
    pub compact_text: String,
    pub raw_text_ref: String,
}

/// Extract the latest `compact_context` from session entries, newest first.
pub fn latest_compact_context(entries: &[SessionTreeEntry]) -> Option<CompactContext> {
    entries.iter().rev().find_map(compact_context_from_entry)
}

/// Extract a `compact_context` from a single session entry.
pub fn compact_context_from_entry(entry: &SessionTreeEntry) -> Option<CompactContext> {
    let SessionTreeEntry::Custom {
        custom_type, data, ..
    } = entry
    else {
        return None;
    };
    if custom_type != COMPACT_CONTEXT_CUSTOM_TYPE {
        return None;
    }
    parse_compact_context(data.as_ref()?)
}

/// Return the non-empty compact text for an entry, if present.
pub fn compact_context_text(entry: &SessionTreeEntry) -> Option<String> {
    compact_context_from_entry(entry)
        .map(|context| context.compact_text)
        .filter(|text| !text.trim().is_empty())
}

/// Parse `compact_context` data, tolerating both `compactText` and legacy `text`.
fn parse_compact_context(data: &Value) -> Option<CompactContext> {
    let source_session_id = data
        .get("sourceSessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let compact_text = data
        .get("compactText")
        .and_then(Value::as_str)
        .or_else(|| data.get("text").and_then(Value::as_str))
        .unwrap_or_default()
        .to_string();
    let raw_text_ref = data
        .get("rawTextRef")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if source_session_id.is_empty() && compact_text.trim().is_empty() && raw_text_ref.is_empty() {
        None
    } else {
        Some(CompactContext {
            source_session_id,
            compact_text,
            raw_text_ref,
        })
    }
}
