use std::collections::BTreeMap;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionModelContextPlacement,
};
use thiserror::Error;

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
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ExtensionModelContextProjectionError {
    #[error("persistent model-context entry is invalid: {0}")]
    InvalidEntry(String),
}
