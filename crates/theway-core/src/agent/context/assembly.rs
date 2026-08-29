//! Assembly of session entries into the LLM `AgentMessage` list.

use crate::agent::context::collapse::{COMPACT_CONTEXT_CUSTOM_TYPE, compact_context_text};
use crate::agent::messages::{branch_summary, compaction_summary, custom};
use crate::agent::session::session::{SessionContext, SessionContextModel, SessionTreeEntry};
use crate::types::AgentMessage;

/// Replay session entries into the in-memory context used by the agent loop.
pub fn build_session_context(path_entries: &[SessionTreeEntry]) -> SessionContext {
    let mut thinking_level = String::from("off");
    let mut model: Option<SessionContextModel> = None;
    let mut compaction_idx: Option<usize> = None;

    for (i, entry) in path_entries.iter().enumerate() {
        match entry {
            SessionTreeEntry::ThinkingLevelChange {
                thinking_level: t, ..
            } => {
                thinking_level = t.clone();
            }
            SessionTreeEntry::ModelChange {
                provider, model_id, ..
            } => {
                model = Some(SessionContextModel {
                    provider: provider.clone(),
                    model_id: model_id.clone(),
                });
            }
            SessionTreeEntry::Message {
                message: AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)),
                ..
            } => {
                model = Some(SessionContextModel {
                    provider: a.provider.0.clone(),
                    model_id: a.model.clone(),
                });
            }
            SessionTreeEntry::Compaction { .. } => {
                compaction_idx = Some(i);
            }
            _ => {}
        }
    }

    let mut messages: Vec<AgentMessage> = Vec::new();
    let append = |messages: &mut Vec<AgentMessage>, entry: &SessionTreeEntry| match entry {
        SessionTreeEntry::Message { message, .. } => messages.push(message.clone()),
        SessionTreeEntry::CustomMessage {
            custom_type,
            content,
            details,
            timestamp,
            ..
        } => {
            let ts = chrono::DateTime::parse_from_rfc3339(timestamp)
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis());
            messages.push(custom(
                custom_type.clone(),
                serde_json::json!({ "content": content, "details": details, "timestamp": ts }),
            ));
        }
        SessionTreeEntry::BranchSummary { summary, .. } if !summary.is_empty() => {
            messages.push(branch_summary(summary.clone()));
        }
        SessionTreeEntry::Custom { custom_type, .. }
            if custom_type == COMPACT_CONTEXT_CUSTOM_TYPE =>
        {
            if let Some(text) = compact_context_text(entry) {
                messages.push(AgentMessage::Custom(crate::types::CustomMessage {
                    role: "collapse_context".into(),
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    payload: serde_json::json!({ "summary": text }),
                }));
            }
        }
        _ => {}
    };

    if let Some(idx) = compaction_idx {
        let SessionTreeEntry::Compaction {
            summary,
            first_kept_entry_id,
            ..
        } = &path_entries[idx]
        else {
            unreachable!()
        };
        messages.push(compaction_summary(summary.clone()));
        let mut found_first_kept = false;
        for (i, entry) in path_entries.iter().enumerate() {
            if i >= idx {
                break;
            }
            if entry.id() == first_kept_entry_id.as_str() {
                found_first_kept = true;
            }
            if found_first_kept {
                append(&mut messages, entry);
            }
        }
        for entry in &path_entries[idx + 1..] {
            append(&mut messages, entry);
        }
    } else {
        for entry in path_entries {
            append(&mut messages, entry);
        }
    }

    SessionContext {
        messages,
        thinking_level,
        model,
    }
}
