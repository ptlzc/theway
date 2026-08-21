use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;
use theway_contract::extension::{
    ExtensionActionBatch, ExtensionActionKind, ExtensionDurableEntry, ExtensionDurableEntryPayload,
    ExtensionLifecycleEvent, ExtensionModelContextPlacement, ExtensionPermission,
    ExtensionStateMutation,
};
use theway_core::agent::runtime_extensions::{
    ExtensionModelContextProjection, SessionExtensionStatePort,
};

use super::catalog::ExtensionPackage;
use super::dispatcher;
use super::engine::{EngineInstanceKey, QuickJsEnginePool};

#[derive(Clone)]
pub(super) struct ExtensionStateRuntime {
    session_id: String,
    port: Arc<dyn SessionExtensionStatePort>,
    engine: QuickJsEnginePool,
    projections: Arc<parking_lot::Mutex<BTreeMap<String, DurableProjection>>>,
    model_context: ExtensionModelContextProjection,
    limits: ExtensionStateLimits,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ExtensionStateLimits {
    pub max_entries_per_batch: usize,
    pub max_entry_bytes: usize,
    pub max_extension_bytes: usize,
}

impl Default for ExtensionStateLimits {
    fn default() -> Self {
        Self {
            max_entries_per_batch: 32,
            max_entry_bytes: 64 * 1024,
            max_extension_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Default)]
struct DurableProjection {
    entries: Vec<ExtensionDurableEntry>,
    state: BTreeMap<String, Value>,
    events: BTreeMap<String, (String, Value)>,
    contexts: BTreeMap<String, (ExtensionModelContextPlacement, Value)>,
    serialized_bytes: usize,
    schema_version: u32,
    target_schema: Option<u32>,
    can_write: bool,
}

impl ExtensionStateRuntime {
    pub(super) fn new(
        session_id: impl Into<String>,
        port: Arc<dyn SessionExtensionStatePort>,
        engine: QuickJsEnginePool,
        limits: ExtensionStateLimits,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            port,
            engine,
            projections: Arc::new(parking_lot::Mutex::new(BTreeMap::new())),
            model_context: ExtensionModelContextProjection::default(),
            limits,
        }
    }

    pub(super) fn model_context(&self) -> ExtensionModelContextProjection {
        self.model_context.clone()
    }

    pub(super) fn entries_for(&self, extension_id: &str) -> Vec<ExtensionDurableEntry> {
        self.projections
            .lock()
            .get(extension_id)
            .map(|projection| projection.entries.clone())
            .unwrap_or_default()
    }

    pub(super) async fn reconstruct(&self, package: &ExtensionPackage) -> Result<(), String> {
        let extension_id = &package.manifest().id;
        let entries = self
            .port
            .replay_durable_entries(extension_id, None)
            .await
            .map_err(|error| error.to_string())?;
        if !entries.is_empty() && package.manifest().state_schema.is_none() {
            return Err("persisted extension state requires manifest stateSchema".into());
        }
        let mut projection = DurableProjection::rebuild(extension_id, entries)?;
        if package
            .manifest()
            .state_schema
            .is_some_and(|schema| projection.schema_version > schema)
        {
            return Err("persisted extension state schema is newer than the package".into());
        }
        projection.target_schema = package.manifest().state_schema;
        projection.can_write = package
            .granted_permissions()
            .contains(&ExtensionPermission::SessionWrite);
        self.projections
            .lock()
            .insert(extension_id.clone(), projection.clone());
        self.engine.install_extension_state(
            &EngineInstanceKey::new(&self.session_id, extension_id),
            package.manifest().state_schema,
            &projection.entries,
        );
        self.rebuild_model_context()?;
        Ok(())
    }

    pub(super) async fn commit_batch(
        &self,
        extension_id: &str,
        origin_sequence: u64,
        batch: &mut ExtensionActionBatch,
    ) -> Result<(), String> {
        let durable_actions = batch
            .actions
            .iter()
            .filter(|action| is_durable(action.kind))
            .collect::<Vec<_>>();
        if durable_actions.is_empty() {
            return Ok(());
        }
        if durable_actions.len() > self.limits.max_entries_per_batch {
            return Err("durable action count exceeds the configured limit".into());
        }
        let (schema, can_write) = self
            .projections
            .lock()
            .get(extension_id)
            .map(|projection| (projection.target_schema, projection.can_write))
            .unwrap_or((None, false));
        if !can_write {
            return Err("durable extension actions require session.write".into());
        }
        let schema = schema
            .ok_or_else(|| "durable extension actions require manifest stateSchema".to_string())?;
        let mut entries = Vec::with_capacity(durable_actions.len());
        for action in durable_actions {
            let entry: ExtensionDurableEntry = serde_json::from_value(action.payload.clone())
                .map_err(|error| format!("invalid durable extension action: {error}"))?;
            validate_owner(&entry, extension_id, schema, origin_sequence)?;
            let bytes = serialized_len(&entry)?;
            if bytes > self.limits.max_entry_bytes {
                return Err("durable extension entry exceeds the configured size limit".into());
            }
            entries.push(entry);
        }
        self.commit_entries(extension_id, entries).await?;
        batch.actions.retain(|action| !is_durable(action.kind));
        Ok(())
    }

    pub(super) async fn migrate_if_needed(
        &self,
        package: &ExtensionPackage,
        key: &EngineInstanceKey,
        metadata: &Value,
        origin_sequence: u64,
        timeout: Duration,
        broker_operation_limit: usize,
    ) -> Result<(), String> {
        let Some(target_schema) = package.manifest().state_schema else {
            return Ok(());
        };
        let (from_schema, state) = {
            let projections = self.projections.lock();
            let projection = projections
                .get(&package.manifest().id)
                .cloned()
                .unwrap_or_default();
            (projection.schema_version, projection.state)
        };
        if from_schema == 0 || from_schema == target_schema {
            return Ok(());
        }
        if from_schema > target_schema {
            return Err("persisted extension state schema is newer than the package".into());
        }
        if !package
            .granted_permissions()
            .contains(&ExtensionPermission::SessionWrite)
        {
            return Err("state migration requires session.write".into());
        }
        let registration_id = metadata
            .get("migrationRegistrationId")
            .and_then(Value::as_u64)
            .ok_or_else(|| "older extension state requires api.migrateState".to_string())?;
        let envelope = dispatcher::envelope(
            &package.manifest().id,
            &key.session_id,
            package.workspace_root().to_string_lossy().as_ref(),
            origin_sequence,
            ExtensionLifecycleEvent::ExtensionLoad,
            serde_json::json!({
                "fromSchemaVersion": from_schema,
                "toSchemaVersion": target_schema,
                "state": state,
            }),
        );
        let result = self
            .engine
            .invoke_controlled_with_effects(
                key,
                &envelope,
                registration_id,
                timeout,
                Arc::new(AtomicBool::new(false)),
                broker_operation_limit,
            )
            .await
            .map_err(|error| error.message)?;
        if !result.queued_durable_actions.is_empty() {
            return Err("state migration must return a replacement state object".into());
        }
        let migrated = result
            .value
            .get("state")
            .and_then(Value::as_object)
            .ok_or_else(|| "state migration must return { state: object }".to_string())?;
        let mut entries = Vec::new();
        for old_key in state.keys() {
            if !migrated.contains_key(old_key) {
                entries.push(state_entry(
                    &package.manifest().id,
                    target_schema,
                    origin_sequence,
                    old_key,
                    ExtensionStateMutation::Delete,
                ));
            }
        }
        for (state_key, value) in migrated {
            entries.push(state_entry(
                &package.manifest().id,
                target_schema,
                origin_sequence,
                state_key,
                ExtensionStateMutation::Set {
                    value: value.clone(),
                },
            ));
        }
        entries.push(ExtensionDurableEntry {
            extension_id: package.manifest().id.clone(),
            state_schema_version: target_schema,
            origin_sequence,
            entry: ExtensionDurableEntryPayload::StateMigration {
                from_schema_version: from_schema,
                to_schema_version: target_schema,
            },
        });
        if entries.len() > self.limits.max_entries_per_batch {
            return Err("state migration entry count exceeds the configured limit".into());
        }
        for entry in &entries {
            entry.validate().map_err(|error| error.to_string())?;
            if serialized_len(entry)? > self.limits.max_entry_bytes {
                return Err("state migration entry exceeds the configured size limit".into());
            }
        }
        self.commit_entries(&package.manifest().id, entries).await
    }

    async fn commit_entries(
        &self,
        extension_id: &str,
        entries: Vec<ExtensionDurableEntry>,
    ) -> Result<(), String> {
        let accepted = {
            let projections = self.projections.lock();
            projections
                .get(extension_id)
                .cloned()
                .unwrap_or_default()
                .accepted_entries(&entries)?
        };
        if accepted.is_empty() {
            return Ok(());
        }
        let accepted_bytes = accepted
            .iter()
            .map(serialized_len)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum::<usize>();
        let current_bytes = self
            .projections
            .lock()
            .get(extension_id)
            .map(|projection| projection.serialized_bytes)
            .unwrap_or_default();
        if current_bytes.saturating_add(accepted_bytes) > self.limits.max_extension_bytes {
            return Err("extension durable state exceeds the configured session quota".into());
        }
        self.port
            .append_durable_entries(extension_id, accepted.clone())
            .await
            .map_err(|error| error.to_string())?;
        {
            let mut projections = self.projections.lock();
            let projection = projections.entry(extension_id.to_string()).or_default();
            projection.apply(&accepted)?;
        }
        self.engine.apply_extension_state(
            &EngineInstanceKey::new(&self.session_id, extension_id),
            &accepted,
        );
        self.rebuild_model_context()?;
        Ok(())
    }

    fn rebuild_model_context(&self) -> Result<(), String> {
        let entries = self
            .projections
            .lock()
            .values()
            .flat_map(|projection| projection.entries.clone())
            .collect::<Vec<_>>();
        self.model_context
            .replace(entries)
            .map_err(|error| error.to_string())
    }
}

impl DurableProjection {
    fn rebuild(extension_id: &str, entries: Vec<ExtensionDurableEntry>) -> Result<Self, String> {
        let mut projection = Self::default();
        for entry in &entries {
            entry.validate().map_err(|error| error.to_string())?;
            if entry.extension_id != extension_id {
                return Err("persisted extension state owner mismatch".into());
            }
        }
        projection.apply(&entries)?;
        Ok(projection)
    }

    fn accepted_entries(
        &self,
        entries: &[ExtensionDurableEntry],
    ) -> Result<Vec<ExtensionDurableEntry>, String> {
        let mut candidate = self.clone();
        let mut accepted = Vec::new();
        for entry in entries {
            if candidate.is_idempotent(entry)? {
                continue;
            }
            candidate.apply(std::slice::from_ref(entry))?;
            accepted.push(entry.clone());
        }
        Ok(accepted)
    }

    fn is_idempotent(&self, entry: &ExtensionDurableEntry) -> Result<bool, String> {
        match &entry.entry {
            ExtensionDurableEntryPayload::StateMutation { key, mutation } => Ok(match mutation {
                ExtensionStateMutation::Set { value } => self.state.get(key) == Some(value),
                ExtensionStateMutation::Delete => !self.state.contains_key(key),
            }),
            ExtensionDurableEntryPayload::CustomEvent {
                event_id,
                custom_type,
                payload,
            } => match self.events.get(event_id) {
                Some((existing_type, existing_payload))
                    if existing_type == custom_type && existing_payload == payload =>
                {
                    Ok(true)
                }
                Some(_) => Err("custom event id already exists with different content".into()),
                None => Ok(false),
            },
            ExtensionDurableEntryPayload::ModelContext {
                context_id,
                placement,
                content,
            } => Ok(self.contexts.get(context_id) == Some(&(*placement, content.clone()))),
            ExtensionDurableEntryPayload::StateMigration { .. } => Ok(false),
        }
    }

    fn apply(&mut self, entries: &[ExtensionDurableEntry]) -> Result<(), String> {
        for entry in entries {
            self.serialized_bytes = self.serialized_bytes.saturating_add(serialized_len(entry)?);
            self.schema_version = self.schema_version.max(entry.state_schema_version);
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
                } => {
                    if let Some((existing_type, existing_payload)) = self.events.get(event_id)
                        && (existing_type != custom_type || existing_payload != payload)
                    {
                        return Err("custom event id has conflicting historical content".into());
                    }
                    self.events
                        .insert(event_id.clone(), (custom_type.clone(), payload.clone()));
                }
                ExtensionDurableEntryPayload::ModelContext {
                    context_id,
                    placement,
                    content,
                } => {
                    self.contexts
                        .insert(context_id.clone(), (*placement, content.clone()));
                }
                ExtensionDurableEntryPayload::StateMigration { .. } => {}
            }
            self.entries.push(entry.clone());
        }
        Ok(())
    }
}

fn validate_owner(
    entry: &ExtensionDurableEntry,
    extension_id: &str,
    schema: u32,
    origin_sequence: u64,
) -> Result<(), String> {
    entry.validate().map_err(|error| error.to_string())?;
    if entry.extension_id != extension_id {
        return Err("durable extension action owner mismatch".into());
    }
    if entry.state_schema_version != schema {
        return Err("durable extension action state schema mismatch".into());
    }
    if entry.origin_sequence != origin_sequence.max(1) {
        return Err("durable extension action origin sequence mismatch".into());
    }
    Ok(())
}

fn state_entry(
    extension_id: &str,
    schema: u32,
    origin_sequence: u64,
    key: &str,
    mutation: ExtensionStateMutation,
) -> ExtensionDurableEntry {
    ExtensionDurableEntry {
        extension_id: extension_id.into(),
        state_schema_version: schema,
        origin_sequence,
        entry: ExtensionDurableEntryPayload::StateMutation {
            key: key.into(),
            mutation,
        },
    }
}

fn is_durable(kind: ExtensionActionKind) -> bool {
    matches!(
        kind,
        ExtensionActionKind::SetState
            | ExtensionActionKind::DeleteState
            | ExtensionActionKind::AppendCustomEvent
            | ExtensionActionKind::AppendModelContext
    )
}

fn serialized_len(entry: &ExtensionDurableEntry) -> Result<usize, String> {
    serde_json::to_vec(entry)
        .map(|serialized| serialized.len())
        .map_err(|error| format!("serialize durable extension entry: {error}"))
}
