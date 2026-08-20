use std::collections::{BTreeMap, BTreeSet};

use parking_lot::Mutex;
use serde_json::{Value, json};
use theway_contract::extension::ExtensionScope;

use super::registrations::EffectRegistration;

/// Lifecycle phase for one session-owned package instance. Registration
/// effects will share this owner boundary and are disposed before the phase
/// becomes `Disposed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InstanceLifecyclePhase {
    Loaded,
    Started,
    Disposed,
}

#[derive(Default)]
struct HealthState {
    consecutive_failures: usize,
    circuit_open: bool,
}

/// Per-session health state shared by synchronous dispatch and queued
/// observations for one extension instance.
#[derive(Default)]
pub(super) struct InstanceHealth {
    state: Mutex<HealthState>,
}

impl InstanceHealth {
    pub(super) fn is_open(&self) -> bool {
        self.state.lock().circuit_open
    }

    pub(super) fn record_success(&self) {
        self.state.lock().consecutive_failures = 0;
    }

    /// Returns true only for the transition that opens the circuit.
    pub(super) fn record_failure(&self, threshold: usize) -> bool {
        let mut state = self.state.lock();
        if state.circuit_open {
            return false;
        }
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
        if state.consecutive_failures < threshold {
            return false;
        }
        state.circuit_open = true;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EffectKind {
    Hook,
    Tool,
    Command,
    Provider,
    PromptSection,
    RequestPolicy,
    Contribution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectOwner {
    pub extension_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectScopeBinding {
    pub scope: ExtensionScope,
    pub run_id: Option<String>,
    pub request_id: Option<String>,
}

impl EffectScopeBinding {
    pub fn setup(scope: ExtensionScope) -> Self {
        Self {
            scope,
            run_id: None,
            request_id: None,
        }
    }

    pub fn bound(
        scope: ExtensionScope,
        run_id: Option<String>,
        request_id: Option<String>,
    ) -> Result<Self, EffectLedgerError> {
        if scope == ExtensionScope::Run && run_id.is_none() {
            return Err(EffectLedgerError::MissingScopeId("run"));
        }
        if scope == ExtensionScope::Request && request_id.is_none() {
            return Err(EffectLedgerError::MissingScopeId("request"));
        }
        Ok(Self {
            scope,
            run_id,
            request_id,
        })
    }
}

#[derive(Clone, Debug)]
pub struct EffectRecord {
    pub handle: u64,
    pub owner: EffectOwner,
    pub kind: EffectKind,
    pub scope: EffectScopeBinding,
    pub conflict_key: String,
    pub restoration_data: Option<Value>,
    pub registration: EffectRegistration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectDisposeOutcome {
    Disposed,
    AlreadyDisposed,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum EffectLedgerError {
    #[error("registration conflicts with the active {kind:?} effect '{key}'")]
    Conflict { kind: EffectKind, key: String },
    #[error("registration override is not authorized")]
    OverrideDenied,
    #[error("registration requires a {0} scope identifier")]
    MissingScopeId(&'static str),
    #[error("registration handle is unknown")]
    UnknownHandle,
    #[error("registration handle is disposed")]
    DisposedHandle,
}

#[derive(Default)]
struct EffectLedgerState {
    next_handle: u64,
    records: BTreeMap<u64, EffectRecord>,
    stacks: BTreeMap<(EffectKind, String), Vec<u64>>,
    acceptance_order: Vec<u64>,
    disposed: BTreeSet<u64>,
}

/// Thread-safe reversible registration ledger. Conflict stacks retain the
/// displaced handle, so disposing an authorized override restores its owner.
#[derive(Clone, Default)]
pub struct EffectLedger {
    state: std::sync::Arc<Mutex<EffectLedgerState>>,
}

impl EffectLedger {
    pub fn active_count(&self) -> usize {
        self.state.lock().records.len()
    }

    pub fn accept(
        &self,
        owner: EffectOwner,
        scope: EffectScopeBinding,
        registration: EffectRegistration,
        override_authorized: bool,
    ) -> Result<u64, EffectLedgerError> {
        let kind = registration.kind();
        let conflict_key = registration.conflict_key();
        let wants_override = registration.requests_override();
        let mut state = self.state.lock();
        let stack_key = (kind, conflict_key.clone());
        let displaced = state
            .stacks
            .get(&stack_key)
            .and_then(|stack| stack.last())
            .copied();
        if displaced.is_some() && !wants_override {
            return Err(EffectLedgerError::Conflict {
                kind,
                key: conflict_key,
            });
        }
        if wants_override && !override_authorized {
            return Err(EffectLedgerError::OverrideDenied);
        }
        state.next_handle = state.next_handle.saturating_add(1);
        let handle = state.next_handle;
        let restoration_data = displaced.map(|value| json!({ "displacedHandle": value }));
        let record = EffectRecord {
            handle,
            owner,
            kind,
            scope,
            conflict_key: conflict_key.clone(),
            restoration_data,
            registration,
        };
        state.records.insert(handle, record);
        state.stacks.entry(stack_key).or_default().push(handle);
        state.acceptance_order.push(handle);
        Ok(handle)
    }

    pub fn active(&self, kind: EffectKind, key: &str) -> Option<EffectRecord> {
        let state = self.state.lock();
        let handle = state
            .stacks
            .get(&(kind, key.to_string()))?
            .last()
            .copied()?;
        state.records.get(&handle).cloned()
    }

    pub fn active_records(&self, kind: EffectKind) -> Vec<EffectRecord> {
        let state = self.state.lock();
        state
            .stacks
            .iter()
            .filter(|((record_kind, _), _)| *record_kind == kind)
            .filter_map(|(_, stack)| stack.last())
            .filter_map(|handle| state.records.get(handle))
            .cloned()
            .collect()
    }

    pub fn record(&self, handle: u64) -> Result<EffectRecord, EffectLedgerError> {
        let state = self.state.lock();
        if state.disposed.contains(&handle) {
            return Err(EffectLedgerError::DisposedHandle);
        }
        state
            .records
            .get(&handle)
            .cloned()
            .ok_or(EffectLedgerError::UnknownHandle)
    }

    pub fn records_for_owner(&self, owner: &EffectOwner) -> Vec<EffectRecord> {
        let state = self.state.lock();
        state
            .acceptance_order
            .iter()
            .rev()
            .filter_map(|handle| state.records.get(handle))
            .filter(|record| &record.owner == owner)
            .cloned()
            .collect()
    }

    pub fn records_for_scope(
        &self,
        scope: ExtensionScope,
        scope_id: Option<&str>,
    ) -> Vec<EffectRecord> {
        let state = self.state.lock();
        state
            .acceptance_order
            .iter()
            .rev()
            .filter_map(|handle| state.records.get(handle))
            .filter(|record| {
                record.scope.scope == scope
                    && match scope {
                        ExtensionScope::Run => scope_id.is_none_or(|id| {
                            record
                                .scope
                                .run_id
                                .as_deref()
                                .is_none_or(|value| value == id)
                        }),
                        ExtensionScope::Request => scope_id.is_none_or(|id| {
                            record
                                .scope
                                .request_id
                                .as_deref()
                                .is_none_or(|value| value == id)
                        }),
                        ExtensionScope::Process | ExtensionScope::Session => true,
                    }
            })
            .cloned()
            .collect()
    }

    pub fn set_restoration_data(&self, handle: u64, value: Value) -> Result<(), EffectLedgerError> {
        let mut state = self.state.lock();
        if state.disposed.contains(&handle) {
            return Err(EffectLedgerError::DisposedHandle);
        }
        let record = state
            .records
            .get_mut(&handle)
            .ok_or(EffectLedgerError::UnknownHandle)?;
        record.restoration_data = Some(value);
        Ok(())
    }

    pub fn dispose(&self, handle: u64) -> Result<EffectDisposeOutcome, EffectLedgerError> {
        let mut state = self.state.lock();
        if state.disposed.contains(&handle) {
            return Ok(EffectDisposeOutcome::AlreadyDisposed);
        }
        let record = state
            .records
            .remove(&handle)
            .ok_or(EffectLedgerError::UnknownHandle)?;
        let stack_key = (record.kind, record.conflict_key);
        if let Some(stack) = state.stacks.get_mut(&stack_key) {
            stack.retain(|candidate| *candidate != handle);
            if stack.is_empty() {
                state.stacks.remove(&stack_key);
            }
        }
        state.disposed.insert(handle);
        Ok(EffectDisposeOutcome::Disposed)
    }

    pub fn dispose_owner(&self, owner: &EffectOwner) -> Vec<u64> {
        self.dispose_matching(|record| &record.owner == owner)
    }

    pub fn dispose_scope(&self, scope: ExtensionScope, scope_id: Option<&str>) -> Vec<u64> {
        self.dispose_matching(|record| {
            if record.scope.scope != scope {
                return false;
            }
            match scope {
                ExtensionScope::Run => scope_id.is_none_or(|value| {
                    record.scope.run_id.as_deref().is_none_or(|id| id == value)
                }),
                ExtensionScope::Request => scope_id.is_none_or(|value| {
                    record
                        .scope
                        .request_id
                        .as_deref()
                        .is_none_or(|id| id == value)
                }),
                ExtensionScope::Process | ExtensionScope::Session => true,
            }
        })
    }

    pub fn dispose_all(&self) -> Vec<u64> {
        self.dispose_matching(|_| true)
    }

    fn dispose_matching(&self, predicate: impl Fn(&EffectRecord) -> bool) -> Vec<u64> {
        let handles = {
            let state = self.state.lock();
            state
                .acceptance_order
                .iter()
                .rev()
                .filter_map(|handle| state.records.get(handle))
                .filter(|record| predicate(record))
                .map(|record| record.handle)
                .collect::<Vec<_>>()
        };
        for handle in &handles {
            let _ = self.dispose(*handle);
        }
        handles
    }
}
