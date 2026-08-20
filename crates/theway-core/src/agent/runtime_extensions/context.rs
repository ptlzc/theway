use std::collections::BTreeMap;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionModelContextPlacement,
};
use thiserror::Error;

use crate::types::{AgentMessage, CustomMessage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionModelContextItem {
    pub extension_id: String,
    pub context_id: String,
    pub placement: ExtensionModelContextPlacement,
    pub content: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionModelContextProjection {
    items: Vec<ExtensionModelContextItem>,
}

impl ExtensionModelContextProjection {
    pub fn rebuild(
        entries: impl IntoIterator<Item = ExtensionDurableEntry>,
    ) -> Result<Self, ExtensionModelContextProjectionError> {
        let mut positions = BTreeMap::<(String, String), usize>::new();
        let mut items = Vec::new();
        for entry in entries {
            entry.validate().map_err(|error| {
                ExtensionModelContextProjectionError::InvalidEntry(error.to_string())
            })?;
            let ExtensionDurableEntryPayload::ModelContext {
                context_id,
                placement,
                content,
            } = entry.entry
            else {
                continue;
            };
            let key = (entry.extension_id.clone(), context_id.clone());
            let projected = ExtensionModelContextItem {
                extension_id: entry.extension_id,
                context_id,
                placement,
                content,
            };
            if let Some(index) = positions.get(&key).copied() {
                items[index] = projected;
            } else {
                positions.insert(key, items.len());
                items.push(projected);
            }
        }
        Ok(Self { items })
    }

    pub fn items(&self) -> &[ExtensionModelContextItem] {
        &self.items
    }

    pub fn into_items(self) -> Vec<ExtensionModelContextItem> {
        self.items
    }

    /// Project the de-duplicated model-visible entries into one compaction-only
    /// message list. Private state and custom events never enter this list.
    pub fn compaction_messages(&self) -> Vec<AgentMessage> {
        self.items
            .iter()
            .map(|item| match item.placement {
                ExtensionModelContextPlacement::Message => {
                    serde_json::from_value(item.content.clone())
                        .unwrap_or_else(|_| model_context_marker(item, "message"))
                }
                ExtensionModelContextPlacement::SystemPromptSection => {
                    model_context_marker(item, "system_prompt_section")
                }
            })
            .collect()
    }
}

fn model_context_marker(item: &ExtensionModelContextItem, placement: &str) -> AgentMessage {
    AgentMessage::Custom(CustomMessage {
        role: "extension_model_context".into(),
        timestamp: 0,
        payload: serde_json::json!({
            "extensionId": item.extension_id,
            "contextId": item.context_id,
            "placement": placement,
            "content": item.content,
        }),
    })
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionModelContextProjectionError {
    #[error("persistent model-context entry is invalid: {0}")]
    InvalidEntry(String),
}
