//! Custom AgentMessage variant helpers. TODO: 1:1 port of
//! `packages/agent/src/harness/messages.ts` (~164 lines). The harness defines stock custom
//! variants used by compaction/branch summarization (`compaction_summary`, `branch_summary`,
//! `custom`).

use crate::types::{AgentMessage, CustomMessage};

pub fn compaction_summary(summary: impl Into<String>) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: "compaction_summary".into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        payload: serde_json::json!({ "summary": summary.into() }),
    })
}

pub fn branch_summary(summary: impl Into<String>) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: "branch_summary".into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        payload: serde_json::json!({ "summary": summary.into() }),
    })
}

pub fn custom(role: impl Into<String>, payload: serde_json::Value) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: role.into(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        payload,
    })
}
