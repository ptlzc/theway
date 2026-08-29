//! Session metadata and collapse-material helpers for [`super::AppSessionOps`].

use std::collections::HashMap;

use anyhow::Result;
use chrono::Utc;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};
use theway_llm_provider::{ContentBlock, Message, UserContent, UserContentBlock};

/// Read the latest `session_metadata` custom entry from a session transcript.
pub(crate) async fn read_session_metadata(
    session: &(impl SessionReader + ?Sized),
) -> Result<HashMap<String, String>> {
    let entries = session.find_entries("custom").await?;
    let mut metadata = HashMap::new();
    for entry in entries {
        if entry
            .payload
            .get("customType")
            .and_then(serde_json::Value::as_str)
            != Some("session_metadata")
        {
            continue;
        }
        if let Some(map) = entry
            .payload
            .get("metadata")
            .and_then(|value| serde_json::from_value::<HashMap<String, String>>(value.clone()).ok())
        {
            metadata.extend(map);
        }
    }
    Ok(metadata)
}

/// Persist a metadata map as a new `session_metadata` custom transcript entry.
pub(super) async fn append_session_metadata(
    session: &(impl SessionStore + ?Sized),
    metadata: &HashMap<String, String>,
) -> Result<()> {
    let id = session.create_entry_id().await?;
    let parent_id = session.get_leaf_id().await?;
    let timestamp = chrono::Utc::now().to_rfc3339();
    let entry = StoredSessionEntry::from_payload(serde_json::json!({
        "type": "custom",
        "id": id,
        "parentId": parent_id,
        "timestamp": timestamp,
        "customType": "session_metadata",
        "metadata": metadata,
    }))?;
    session.append_entry(entry).await?;
    Ok(())
}

pub(super) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Fixed five-component contract for every collapse's bounded rolling
/// summary: `(component name, per-component char cap)`. `critical context`
/// gets the largest budget because it is the catch-all carry-forward slot.
pub(crate) const ROLLING_SUMMARY_COMPONENTS: [(&str, usize); 5] = [
    ("goal", 400),
    ("completed work", 1_200),
    ("key decisions", 600),
    ("next steps", 400),
    ("critical context", 2_400),
];

/// Header recognition for a previously-rendered rolling summary line
/// (`name: rest`). Returns the component index and the text after the colon.
fn rolling_summary_header(line: &str) -> Option<(usize, &str)> {
    for (index, (name, _)) in ROLLING_SUMMARY_COMPONENTS.iter().enumerate() {
        if let Some(rest) = line
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            return Some((index, rest.trim()));
        }
    }
    None
}

/// Bound one component to its char cap. Values are trimmed; truncation is
/// marked explicitly so a reader can tell the component is lossy.
fn bounded_component(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    if text.chars().count() <= limit {
        return text.to_string();
    }
    const MARKER: &str = "… [truncated]";
    let mut bounded: String = text
        .chars()
        .take(limit.saturating_sub(MARKER.chars().count()))
        .collect();
    bounded.push_str(MARKER);
    bounded
}

/// Parse previously rendered rolling-summary components. Plain material
/// (no recognized headers) is carried forward under `critical context`, so
/// no information is silently dropped from a legacy compaction summary.
#[derive(Default)]
struct RollingSummary {
    goal: String,
    completed_work: String,
    key_decisions: String,
    next_steps: String,
    critical_context: String,
}

impl RollingSummary {
    fn from_material(material: &str) -> Self {
        let mut summary = Self::default();
        let mut values: [String; 5] = Default::default();
        let mut current: Option<usize> = None;
        let mut saw_header = false;
        for line in material.lines() {
            if let Some((index, rest)) = rolling_summary_header(line.trim()) {
                saw_header = true;
                current = Some(index);
                values[index] = rest.to_string();
                continue;
            }
            if let Some(index) = current {
                if !values[index].is_empty() {
                    values[index].push('\n');
                }
                values[index].push_str(line.trim());
            }
        }
        if !saw_header {
            summary.critical_context = bounded_component(material, 2_400);
            return summary;
        }
        let goal = std::mem::take(&mut values[0]);
        summary.goal = bounded_component(&goal, 400);
        let completed = std::mem::take(&mut values[1]);
        summary.completed_work = bounded_component(&completed, 1_200);
        let decisions = std::mem::take(&mut values[2]);
        summary.key_decisions = bounded_component(&decisions, 600);
        let next = std::mem::take(&mut values[3]);
        summary.next_steps = bounded_component(&next, 400);
        let critical = std::mem::take(&mut values[4]);
        summary.critical_context = bounded_component(&critical, 2_400);
        summary
    }
}

/// Render the bounded rolling summary: all five fixed components are always
/// present; components without source material stay empty (structure only).
pub(crate) fn render_rolling_summary(material: &str) -> String {
    let material = material.trim();
    if material.is_empty() {
        return String::new();
    }
    let summary = RollingSummary::from_material(material);
    let mut out = String::new();
    out.push_str("goal: ");
    out.push_str(&summary.goal);
    out.push('\n');
    out.push_str("completed work: ");
    out.push_str(&summary.completed_work);
    out.push('\n');
    out.push_str("key decisions: ");
    out.push_str(&summary.key_decisions);
    out.push('\n');
    out.push_str("next steps: ");
    out.push_str(&summary.next_steps);
    out.push('\n');
    out.push_str("critical context: ");
    out.push_str(&summary.critical_context);
    out.push('\n');
    out
}

/// Push one provider text piece into the transcript fallback material.
fn push_provider_text(material: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !material.is_empty() {
        material.push_str("\n\n");
    }
    material.push_str(text);
}

/// Serialize the active transcript into plain text for the
/// `request.summary` → `latest_collapse_summary` → transcript fallback chain.
/// Tool-result bodies are intentionally skipped; user and assistant text is
/// what the previous generation's work was about.
pub(super) async fn transcript_material(session: &theway_core::Session) -> Result<String> {
    let context = session.build_context().await?;
    let provider_messages = theway_core::default_convert_to_llm()(&context.messages);
    let mut material = String::new();
    for message in provider_messages {
        match message {
            Message::User(user) => match user.content {
                UserContent::Text(text) => push_provider_text(&mut material, &text),
                UserContent::Blocks(blocks) => {
                    for block in blocks {
                        if let UserContentBlock::Text(text) = block {
                            push_provider_text(&mut material, &text.text);
                        }
                    }
                }
            },
            Message::Assistant(assistant) => {
                for block in assistant.content {
                    if let ContentBlock::Text(text) = block {
                        push_provider_text(&mut material, &text.text);
                    }
                }
            }
            Message::ToolResult(_) => {}
        }
    }
    Ok(material)
}

pub(super) fn compact_context_entries(
    source_session_id: &str,
    compact_text: &str,
    raw_text_ref: &str,
    graph_state: &theway_core::multiagent::session_graph::SessionGraphState,
    parent_id: Option<&str>,
) -> Result<Vec<StoredSessionEntry>> {
    let timestamp = now_rfc3339();
    let first_id = uuid::Uuid::now_v7().to_string();
    let second_id = uuid::Uuid::now_v7().to_string();
    let compact = StoredSessionEntry::from_payload(serde_json::json!({
        "type": "custom",
        "id": first_id,
        "parentId": parent_id,
        "timestamp": timestamp,
        "customType": "compact_context",
        "data": {
            "sourceSessionId": source_session_id,
            "compactText": compact_text,
            "rawTextRef": raw_text_ref,
            "collapsedAt": timestamp,
        },
    }))?;
    let mut graph_data = serde_json::to_value(graph_state)?;
    if let Some(object) = graph_data.as_object_mut() {
        object
            .entry("dags")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        object
            .entry("subagents")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let graph = StoredSessionEntry::session_graph_state(
        second_id,
        Some(first_id.clone()),
        timestamp,
        graph_data,
    )?;
    Ok(vec![compact, graph])
}
