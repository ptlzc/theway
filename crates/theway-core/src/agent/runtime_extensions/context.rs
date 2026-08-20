use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::RwLock;
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

#[derive(Clone, Debug, Default)]
pub struct ExtensionModelContextProjection {
    items: Arc<RwLock<Vec<ExtensionModelContextItem>>>,
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
        Ok(Self {
            items: Arc::new(RwLock::new(items)),
        })
    }

    pub fn items(&self) -> Vec<ExtensionModelContextItem> {
        self.items.read().clone()
    }

    pub fn into_items(self) -> Vec<ExtensionModelContextItem> {
        self.items.read().clone()
    }

    /// Replace the live branch projection while retaining all shared handles.
    pub fn replace(
        &self,
        entries: impl IntoIterator<Item = ExtensionDurableEntry>,
    ) -> Result<(), ExtensionModelContextProjectionError> {
        let rebuilt = Self::rebuild(entries)?;
        *self.items.write() = rebuilt.items();
        Ok(())
    }

    /// Add the de-duplicated model-visible projection to one normalized model
    /// request. This never mutates the persisted agent transcript.
    pub fn apply_to_request(
        &self,
        request: &mut crate::agent::model_request::NormalizedModelRequestDraft,
    ) {
        let items = self.items.read();
        let sections = items
            .iter()
            .filter_map(|item| {
                (item.placement == ExtensionModelContextPlacement::SystemPromptSection)
                    .then(|| item.content.as_str())
                    .flatten()
            })
            .collect::<Vec<_>>();
        if !sections.is_empty() {
            let suffix = sections.join("\n\n");
            request.system_instructions = Some(match request.system_instructions.take() {
                Some(base) if !base.is_empty() => format!("{base}\n\n{suffix}"),
                _ => suffix,
            });
        }
        request.messages.extend(items.iter().filter_map(|item| {
            if item.placement != ExtensionModelContextPlacement::Message {
                return None;
            }
            let message = serde_json::from_value::<AgentMessage>(item.content.clone()).ok()?;
            match message {
                AgentMessage::Llm(message) => Some(message),
                AgentMessage::Custom(_) => None,
            }
        }));
    }

    /// Project the de-duplicated model-visible entries into one compaction-only
    /// message list. Private state and custom events never enter this list.
    pub fn compaction_messages(&self) -> Vec<AgentMessage> {
        self.items
            .read()
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

impl PartialEq for ExtensionModelContextProjection {
    fn eq(&self, other: &Self) -> bool {
        *self.items.read() == *other.items.read()
    }
}

impl Eq for ExtensionModelContextProjection {}

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
