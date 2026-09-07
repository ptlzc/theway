//! Dynamic trigger rules created at runtime from natural-language user requests.
//!
//! This is intentionally source-agnostic: a rule stores the user's condition as text and
//! lets the trigger action agent evaluate that condition against whatever event envelope
//! arrived. Concrete sources (MCP, future GitHub/webhook/local watchers) only need to emit
//! normal runtime `Trigger`s.
//!
//! Split by domain: [`parse`] (natural-language rule spec parser), [`hooks`] (periodic
//! check hook, action hooks, fire-once listener, prompt rendering), [`tools`]
//! (model-facing CRUD tools).

mod hooks;
mod parse;
mod tools;

#[allow(unused_imports)]
pub use hooks::{
    DynamicTriggerCheckHook, before_trigger_action_hook, direct_inject_action_hook,
    fire_once_trigger_listener,
};
#[allow(unused_imports)]
pub use parse::{ParseTriggerRuleError, ParsedTriggerRule, parse_trigger_rule};
#[allow(unused_imports)]
pub use tools::{ListTriggersTool, NewTriggerTool, RemoveTriggerTool, SetTriggerStateTool};

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
#[cfg(test)]
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The bridged test mirror (`tests/triggers/dynamic/mod.rs`) pulls everything it needs via
// `use super::*`; names it uses but this module's own code does not are re-imported here
// for test builds only.
#[cfg(test)]
#[allow(unused_imports)]
use crate::trigger_engine::execution::{BeforeTriggerActionContext, PromoteAction};
#[cfg(test)]
#[allow(unused_imports)]
use crate::trigger_engine::notification_hook::NotificationHook;
#[cfg(test)]
#[allow(unused_imports)]
use crate::trigger_engine::types::Trigger;
#[cfg(test)]
#[allow(unused_imports)]
use hooks::extract_dynamic_rule_ids;
#[cfg(test)]
#[allow(unused_imports)]
use parse::ZH_WHEN_PREFIX;
#[cfg(test)]
#[allow(unused_imports)]
use theway_core::{AgentTool, PermissionClassification};
// Data model + poll-interval default live in the pure leaf contract crate
// (issue #64); `theway_transport::triggers` re-exports the same items.
pub use theway_contract::triggers::{
    DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS, DynamicTriggerRule,
};
#[cfg(test)]
#[allow(unused_imports)]
use tokio::time::Duration;
#[cfg(test)]
#[allow(unused_imports)]
use tokio_util::sync::CancellationToken;
#[derive(Clone, Debug)]
pub struct DynamicTriggerRegistry {
    inner: Arc<Mutex<DynamicTriggerRegistryState>>,
    poll_interval_secs: Arc<AtomicU64>,
}

impl Default for DynamicTriggerRegistry {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(DynamicTriggerRegistryState::default())),
            poll_interval_secs: Arc::new(AtomicU64::new(
                DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS,
            )),
        }
    }
}

#[derive(Clone, Default)]
enum DynamicTriggerPersistence {
    #[default]
    None,
    Path(PathBuf),
    Runtime {
        storage: Arc<dyn theway_daemon::runtime_storage::RuntimeStorage>,
        cwd: PathBuf,
        session_id: String,
    },
}

impl std::fmt::Debug for DynamicTriggerPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Self::Runtime {
                cwd, session_id, ..
            } => f
                .debug_struct("Runtime")
                .field("cwd", cwd)
                .field("session_id", session_id)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct DynamicTriggerRegistryState {
    rules: Vec<DynamicTriggerRule>,
    storage: DynamicTriggerPersistence,
}

impl DynamicTriggerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_poll_interval_secs(&self, secs: u64) {
        self.poll_interval_secs
            .store(secs.max(1), Ordering::Relaxed);
    }

    pub fn poll_interval_secs(&self) -> u64 {
        self.poll_interval_secs.load(Ordering::Relaxed)
    }

    pub fn load_from_path(
        &self,
        path: impl Into<PathBuf>,
    ) -> Result<(), DynamicTriggerStorageError> {
        let path = path.into();
        let rules = read_rules_file(&path)?;
        let mut state = self.inner.lock();
        state.rules = rules;
        state.storage = DynamicTriggerPersistence::Path(path);
        Ok(())
    }

    /// Load rules from the runtime storage seam (issue #86). Mutations are
    /// persisted back through the same seam; remote storage saves are
    /// fire-and-forget RPC writes.
    pub async fn load_from_storage(
        &self,
        storage: Arc<dyn theway_daemon::runtime_storage::RuntimeStorage>,
        cwd: PathBuf,
        session_id: String,
    ) -> Result<(), DynamicTriggerStorageError> {
        let rules = storage
            .load_dynamic_triggers(&cwd, &session_id)
            .await
            .map_err(|e| DynamicTriggerStorageError::Read(e.to_string()))?;
        let mut state = self.inner.lock();
        state.rules = rules;
        state.storage = DynamicTriggerPersistence::Runtime {
            storage,
            cwd,
            session_id,
        };
        Ok(())
    }

    pub fn storage_path(&self) -> Option<PathBuf> {
        match &self.inner.lock().storage {
            DynamicTriggerPersistence::Path(path) => Some(path.clone()),
            _ => None,
        }
    }

    pub fn add_rule(
        &self,
        condition: &str,
        action: &str,
    ) -> Result<DynamicTriggerRule, AddTriggerRuleError> {
        self.add_rule_with_options(condition, action, true)
    }

    pub fn add_rule_with_options(
        &self,
        condition: &str,
        action: &str,
        fire_once: bool,
    ) -> Result<DynamicTriggerRule, AddTriggerRuleError> {
        self.add_rule_with_flags(condition, action, fire_once, false)
    }

    pub fn add_rule_with_flags(
        &self,
        condition: &str,
        action: &str,
        fire_once: bool,
        promote_to_chat: bool,
    ) -> Result<DynamicTriggerRule, AddTriggerRuleError> {
        let condition = condition.trim();
        let action = action.trim();
        if condition.is_empty() || action.is_empty() {
            return Err(ParseTriggerRuleError::EmptyPart.into());
        }
        let rule = DynamicTriggerRule {
            id: format!("dyn-{}", Uuid::new_v4().simple()),
            condition: condition.to_string(),
            action: action.to_string(),
            enabled: true,
            fire_once,
            fired_at: None,
            promote_to_chat,
            created_at: Utc::now(),
        };
        self.insert_rule(rule)
    }

    pub fn add_from_spec(&self, spec: &str) -> Result<DynamicTriggerRule, AddTriggerRuleError> {
        let parsed = parse_trigger_rule(spec)?;
        self.add_rule(&parsed.condition, &parsed.action)
    }

    fn insert_rule(
        &self,
        rule: DynamicTriggerRule,
    ) -> Result<DynamicTriggerRule, AddTriggerRuleError> {
        let mut state = self.inner.lock();
        let mut next = state.rules.clone();
        next.push(rule.clone());
        persist_rules(&state.storage, &next)?;
        state.rules = next;
        Ok(rule)
    }

    pub fn list(&self) -> Vec<DynamicTriggerRule> {
        self.inner.lock().rules.clone()
    }

    pub fn remove_rule(
        &self,
        id: &str,
    ) -> Result<Option<DynamicTriggerRule>, DynamicTriggerStorageError> {
        let id = id.trim();
        let mut state = self.inner.lock();
        let Some(pos) = state.rules.iter().position(|rule| rule.id == id) else {
            return Ok(None);
        };
        let mut next = state.rules.clone();
        let removed = next.remove(pos);
        persist_rules(&state.storage, &next)?;
        state.rules = next;
        Ok(Some(removed))
    }

    pub fn set_rule_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<DynamicTriggerRule>, DynamicTriggerStorageError> {
        let id = id.trim();
        let mut state = self.inner.lock();
        let Some(pos) = state.rules.iter().position(|rule| rule.id == id) else {
            return Ok(None);
        };
        let mut next = state.rules.clone();
        next[pos].enabled = enabled;
        if enabled {
            next[pos].fired_at = None;
        }
        let updated = next[pos].clone();
        persist_rules(&state.storage, &next)?;
        state.rules = next;
        Ok(Some(updated))
    }

    pub fn clear_rules(&self) -> Result<usize, DynamicTriggerStorageError> {
        let mut state = self.inner.lock();
        let count = state.rules.len();
        if count == 0 {
            return Ok(0);
        }
        persist_rules(&state.storage, &[])?;
        state.rules.clear();
        Ok(count)
    }

    pub fn mark_rules_fired(
        &self,
        ids: &[String],
    ) -> Result<Vec<DynamicTriggerRule>, DynamicTriggerStorageError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut state = self.inner.lock();
        let now = Utc::now();
        let mut next = state.rules.clone();
        let mut changed = Vec::new();
        for rule in &mut next {
            if !rule.fire_once || !rule.enabled || !ids.iter().any(|id| id == &rule.id) {
                continue;
            }
            rule.enabled = false;
            rule.fired_at = Some(now);
            changed.push(rule.clone());
        }
        if changed.is_empty() {
            return Ok(Vec::new());
        }
        persist_rules(&state.storage, &next)?;
        state.rules = next;
        Ok(changed)
    }

    #[allow(dead_code)]
    pub fn clear_for_tests(&self) {
        *self.inner.lock() = DynamicTriggerRegistryState::default();
    }
}

#[cfg(test)]
pub fn global_registry() -> &'static DynamicTriggerRegistry {
    static CELL: OnceCell<DynamicTriggerRegistry> = OnceCell::new();
    CELL.get_or_init(DynamicTriggerRegistry::new)
}

#[cfg(test)]
pub fn set_dynamic_trigger_poll_interval_secs(secs: u64) {
    global_registry().set_poll_interval_secs(secs);
}

#[cfg(test)]
pub fn dynamic_trigger_poll_interval_secs() -> u64 {
    global_registry().poll_interval_secs()
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum AddTriggerRuleError {
    #[error(transparent)]
    Parse(#[from] ParseTriggerRuleError),
    #[error(transparent)]
    Storage(#[from] DynamicTriggerStorageError),
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum DynamicTriggerStorageError {
    #[error("read dynamic triggers: {0}")]
    Read(String),
    #[error("parse dynamic triggers: {0}")]
    Parse(String),
    #[error("write dynamic triggers: {0}")]
    Write(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DynamicTriggerFile {
    version: u32,
    rules: Vec<DynamicTriggerRule>,
}

const DYNAMIC_TRIGGER_FILE_VERSION: u32 = 1;

fn persist_rules(
    storage: &DynamicTriggerPersistence,
    rules: &[DynamicTriggerRule],
) -> Result<(), DynamicTriggerStorageError> {
    match storage {
        DynamicTriggerPersistence::Path(path) => write_rules_file(path, rules),
        DynamicTriggerPersistence::Runtime {
            storage,
            cwd,
            session_id,
        } => {
            let storage = storage.clone();
            let cwd = cwd.clone();
            let session_id = session_id.clone();
            let rules = rules.to_vec();
            tokio::spawn(async move {
                if let Err(e) = storage
                    .save_dynamic_triggers(&cwd, &session_id, &rules)
                    .await
                {
                    tracing::warn!(error = %e, "dynamic trigger remote persist failed");
                }
            });
            Ok(())
        }
        DynamicTriggerPersistence::None => Ok(()),
    }
}

pub(crate) fn read_rules_file(
    path: &Path,
) -> Result<Vec<DynamicTriggerRule>, DynamicTriggerStorageError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(DynamicTriggerStorageError::Read(e.to_string())),
    };
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let file: DynamicTriggerFile = serde_json::from_str(&text)
        .map_err(|e| DynamicTriggerStorageError::Parse(e.to_string()))?;
    Ok(file.rules)
}

pub(crate) fn write_rules_file(
    path: &Path,
    rules: &[DynamicTriggerRule],
) -> Result<(), DynamicTriggerStorageError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DynamicTriggerStorageError::Write(e.to_string()))?;
    }
    let file = DynamicTriggerFile {
        version: DYNAMIC_TRIGGER_FILE_VERSION,
        rules: rules.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file)
        .map_err(|e| DynamicTriggerStorageError::Write(e.to_string()))?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("dynamic-triggers.json");
    let tmp = path.with_file_name(format!("{file_name}.tmp-{}", Uuid::new_v4().simple()));
    std::fs::write(&tmp, text).map_err(|e| DynamicTriggerStorageError::Write(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| DynamicTriggerStorageError::Write(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
// Test files live in `tests/triggers/dynamic/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("triggers/dynamic");

#[cfg(test)]
mod coverage_gap {
    use super::*;

    #[test]
    fn poll_interval_clamps_zero_to_one_second() {
        let registry = DynamicTriggerRegistry::new();
        registry.set_poll_interval_secs(0);
        assert_eq!(registry.poll_interval_secs(), 1);
    }

    #[test]
    fn add_rule_with_flags_rejects_empty_condition_or_action() {
        let registry = DynamicTriggerRegistry::new();
        let err = registry
            .add_rule_with_flags("  ", "echo ok", true, false)
            .unwrap_err();
        assert!(matches!(
            err,
            AddTriggerRuleError::Parse(ParseTriggerRuleError::EmptyPart)
        ));
        let err = registry
            .add_rule_with_flags("always", "  ", true, false)
            .unwrap_err();
        assert!(matches!(
            err,
            AddTriggerRuleError::Parse(ParseTriggerRuleError::EmptyPart)
        ));
    }

    #[test]
    fn clear_rules_noop_and_removal_are_counted() {
        let registry = DynamicTriggerRegistry::new();
        assert_eq!(registry.clear_rules().unwrap(), 0);
        registry.add_rule("a", "b").unwrap();
        assert_eq!(registry.clear_rules().unwrap(), 1);
        assert!(registry.list().is_empty());
    }

    #[test]
    fn mark_rules_fired_ignores_empty_id_list() {
        let registry = DynamicTriggerRegistry::new();
        registry.add_rule("a", "b").unwrap();
        let changed = registry.mark_rules_fired(&[]).unwrap();
        assert!(changed.is_empty());
        assert!(registry.list()[0].enabled);
    }

    #[test]
    fn read_rules_file_treats_empty_file_as_no_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        std::fs::write(&path, "   \n").unwrap();
        assert!(read_rules_file(&path).unwrap().is_empty());
    }

    #[test]
    fn read_rules_file_rejects_malformed_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(matches!(
            read_rules_file(&path),
            Err(DynamicTriggerStorageError::Parse(_))
        ));
    }

    #[tokio::test]
    async fn persist_rules_covers_runtime_and_none_storage_variants() {
        let rules = vec![];
        let none = DynamicTriggerPersistence::None;
        assert!(persist_rules(&none, &rules).is_ok());

        let runtime = DynamicTriggerPersistence::Runtime {
            storage: theway_daemon::runtime_storage::local_runtime_storage(),
            cwd: std::path::PathBuf::from("."),
            session_id: "sess".into(),
        };
        // Fire-and-forget spawn: must return Ok immediately.
        assert!(persist_rules(&runtime, &rules).is_ok());
    }

    #[test]
    fn registry_remove_set_enable_and_mark_fired_cover_both_found_paths() {
        let registry = DynamicTriggerRegistry::new();
        assert!(registry.remove_rule("missing").unwrap().is_none());
        let rule = registry.add_rule("a", "b").unwrap();

        let enabled = registry.set_rule_enabled(&rule.id, false).unwrap().unwrap();
        assert!(!enabled.enabled);
        let enabled = registry.set_rule_enabled(&rule.id, true).unwrap().unwrap();
        assert!(enabled.enabled);

        // fire-once enabled rule with matching id is disabled by mark_rules_fired;
        // non-fire-once and non-matching rules are ignored.
        let repeat = registry.add_rule_with_options("c", "d", false).unwrap();
        let changed = registry
            .mark_rules_fired(&[rule.id.clone(), repeat.id.clone(), "missing".into()])
            .unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].id, rule.id);
        assert!(
            !registry
                .list()
                .iter()
                .find(|r| r.id == rule.id)
                .unwrap()
                .enabled
        );
        assert!(
            registry
                .list()
                .iter()
                .find(|r| r.id == repeat.id)
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn read_rules_file_missing_path_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        assert!(read_rules_file(&path).unwrap().is_empty());
    }
}

#[cfg(test)]
mod extra_tests {
    tests_bridge_macro::tests_bridge!("triggers/dynamic/extra");
}

#[cfg(test)]
mod storage_tests {
    tests_bridge_macro::tests_bridge!("triggers/dynamic/storage_tests");
}
