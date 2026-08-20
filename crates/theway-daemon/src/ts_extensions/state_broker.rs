use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use theway_contract::extension::{
    ExtensionDurableEntry, ExtensionDurableEntryPayload, ExtensionStateMutation,
};

use super::brokers::BrokerError;
use super::engine::EngineInstanceKey;

type InstanceName = (String, String);

#[derive(Clone, Default)]
pub(super) struct ExtensionStateBroker {
    durable: Arc<parking_lot::RwLock<BTreeMap<InstanceName, DurableProjection>>>,
    memory: Arc<parking_lot::RwLock<BTreeMap<InstanceName, BTreeMap<String, Value>>>>,
}

#[derive(Clone, Default)]
struct DurableProjection {
    schema_version: Option<u32>,
    state: BTreeMap<String, Value>,
    events: Vec<Value>,
}

impl ExtensionStateBroker {
    pub(super) fn install(
        &self,
        key: &EngineInstanceKey,
        schema_version: Option<u32>,
        entries: &[ExtensionDurableEntry],
    ) {
        let mut projection = DurableProjection {
            schema_version,
            ..DurableProjection::default()
        };
        projection.apply(entries);
        self.durable.write().insert(instance_name(key), projection);
        self.memory.write().remove(&instance_name(key));
    }

    pub(super) fn apply(&self, key: &EngineInstanceKey, entries: &[ExtensionDurableEntry]) {
        if let Some(projection) = self.durable.write().get_mut(&instance_name(key)) {
            projection.apply(entries);
        }
    }

    pub(super) fn clear_memory(&self, key: &EngineInstanceKey) {
        self.memory.write().remove(&instance_name(key));
    }

    pub(super) fn call(
        &self,
        key: &EngineInstanceKey,
        operation: &str,
        serialized_arguments: &str,
    ) -> Result<Value, BrokerError> {
        match operation {
            "state.schema" => self.schema(key),
            "state.get" => self.state_get(key, parse(serialized_arguments)?),
            "events.replay" => self.events_replay(key, parse(serialized_arguments)?),
            "memory.get" => self.memory_get(key, parse(serialized_arguments)?),
            "memory.set" => self.memory_set(key, parse(serialized_arguments)?),
            "memory.delete" => self.memory_delete(key, parse(serialized_arguments)?),
            "memory.clear" => self.memory_clear(key),
            _ => Err(BrokerError::contract("unknown extension state operation")),
        }
    }

    fn schema(&self, key: &EngineInstanceKey) -> Result<Value, BrokerError> {
        self.durable
            .read()
            .get(&instance_name(key))
            .and_then(|projection| projection.schema_version)
            .map(|version| json!(version))
            .ok_or_else(|| {
                BrokerError::new(
                    "state_schema_required",
                    "extension manifest must declare stateSchema",
                )
            })
    }

    fn state_get(
        &self,
        key: &EngineInstanceKey,
        arguments: KeyArguments,
    ) -> Result<Value, BrokerError> {
        validate_key(&arguments.key)?;
        self.schema(key)?;
        Ok(self
            .durable
            .read()
            .get(&instance_name(key))
            .and_then(|projection| projection.state.get(&arguments.key).cloned())
            .unwrap_or(Value::Null))
    }

    fn events_replay(
        &self,
        key: &EngineInstanceKey,
        arguments: ReplayArguments,
    ) -> Result<Value, BrokerError> {
        self.schema(key)?;
        let events = self
            .durable
            .read()
            .get(&instance_name(key))
            .map(|projection| {
                projection
                    .events
                    .iter()
                    .filter(|event| {
                        arguments.custom_type.as_ref().is_none_or(|expected| {
                            event.get("type").and_then(Value::as_str) == Some(expected)
                        })
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Value::Array(events))
    }

    fn memory_get(
        &self,
        key: &EngineInstanceKey,
        arguments: KeyArguments,
    ) -> Result<Value, BrokerError> {
        validate_key(&arguments.key)?;
        Ok(self
            .memory
            .read()
            .get(&instance_name(key))
            .and_then(|memory| memory.get(&arguments.key).cloned())
            .unwrap_or(Value::Null))
    }

    fn memory_set(
        &self,
        key: &EngineInstanceKey,
        arguments: SetArguments,
    ) -> Result<Value, BrokerError> {
        validate_key(&arguments.key)?;
        self.memory
            .write()
            .entry(instance_name(key))
            .or_default()
            .insert(arguments.key, arguments.value);
        Ok(Value::Null)
    }

    fn memory_delete(
        &self,
        key: &EngineInstanceKey,
        arguments: KeyArguments,
    ) -> Result<Value, BrokerError> {
        validate_key(&arguments.key)?;
        if let Some(memory) = self.memory.write().get_mut(&instance_name(key)) {
            memory.remove(&arguments.key);
        }
        Ok(Value::Null)
    }

    fn memory_clear(&self, key: &EngineInstanceKey) -> Result<Value, BrokerError> {
        self.clear_memory(key);
        Ok(Value::Null)
    }
}

impl DurableProjection {
    fn apply(&mut self, entries: &[ExtensionDurableEntry]) {
        for entry in entries {
            match &entry.entry {
                ExtensionDurableEntryPayload::StateMutation { key, mutation } => match mutation {
                    ExtensionStateMutation::Set { value } => {
                        self.state.insert(key.clone(), value.clone());
                    }
                    ExtensionStateMutation::Delete => {
                        self.state.remove(key);
                    }
                },
                ExtensionDurableEntryPayload::CustomEvent {
                    event_id,
                    custom_type,
                    payload,
                } => self.events.push(json!({
                    "eventId": event_id,
                    "type": custom_type,
                    "payload": payload,
                    "stateSchemaVersion": entry.state_schema_version,
                    "originSequence": entry.origin_sequence,
                })),
                ExtensionDurableEntryPayload::ModelContext { .. }
                | ExtensionDurableEntryPayload::StateMigration { .. } => {}
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyArguments {
    key: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetArguments {
    key: String,
    value: Value,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReplayArguments {
    #[serde(default)]
    custom_type: Option<String>,
}

fn parse<T: for<'de> Deserialize<'de>>(source: &str) -> Result<T, BrokerError> {
    serde_json::from_str(source)
        .map_err(|_| BrokerError::contract("extension state arguments are invalid"))
}

fn validate_key(key: &str) -> Result<(), BrokerError> {
    if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
        Err(BrokerError::contract("extension state key is invalid"))
    } else {
        Ok(())
    }
}

fn instance_name(key: &EngineInstanceKey) -> InstanceName {
    (key.session_id.clone(), key.extension_id.clone())
}
