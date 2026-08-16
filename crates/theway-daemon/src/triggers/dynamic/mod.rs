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
use once_cell::sync::OnceCell;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The bridged test mirror (`tests/triggers/dynamic/mod.rs`) pulls everything it needs via
// `use super::*`; names it uses but this module's own code does not are re-imported here
// for test builds only.
#[cfg(test)]
use crate::trigger_engine::execution::{BeforeTriggerActionContext, PromoteAction};
#[cfg(test)]
use crate::trigger_engine::notification_hook::NotificationHook;
#[cfg(test)]
use crate::trigger_engine::types::Trigger;
#[cfg(test)]
use hooks::extract_dynamic_rule_ids;
#[cfg(test)]
use parse::ZH_WHEN_PREFIX;
#[cfg(test)]
use theway_core::{AgentTool, PermissionClassification};
// Data model + poll-interval default live in the pure leaf contract crate
// (issue #64); `theway_transport::triggers` re-exports the same items.
pub use theway_contract::triggers::{
    DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS, DynamicTriggerRule,
};
#[cfg(test)]
use tokio::time::Duration;
#[cfg(test)]
use tokio_util::sync::CancellationToken;
static CONFIGURED_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS: AtomicU64 =
    AtomicU64::new(DEFAULT_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS);

#[derive(Clone, Debug, Default)]
pub struct DynamicTriggerRegistry {
    inner: Arc<Mutex<DynamicTriggerRegistryState>>,
}

#[derive(Clone, Debug, Default)]
struct DynamicTriggerRegistryState {
    rules: Vec<DynamicTriggerRule>,
    storage_path: Option<PathBuf>,
}

impl DynamicTriggerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load_from_path(
        &self,
        path: impl Into<PathBuf>,
    ) -> Result<(), DynamicTriggerStorageError> {
        let path = path.into();
        let rules = read_rules_file(&path)?;
        let mut state = self.inner.lock();
        state.rules = rules;
        state.storage_path = Some(path);
        Ok(())
    }

    pub fn storage_path(&self) -> Option<PathBuf> {
        self.inner.lock().storage_path.clone()
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
        if let Some(path) = &state.storage_path {
            write_rules_file(path, &next)?;
        }
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
        if let Some(path) = &state.storage_path {
            write_rules_file(path, &next)?;
        }
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
        if let Some(path) = &state.storage_path {
            write_rules_file(path, &next)?;
        }
        state.rules = next;
        Ok(Some(updated))
    }

    pub fn clear_rules(&self) -> Result<usize, DynamicTriggerStorageError> {
        let mut state = self.inner.lock();
        let count = state.rules.len();
        if count == 0 {
            return Ok(0);
        }
        if let Some(path) = &state.storage_path {
            write_rules_file(path, &[])?;
        }
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
        if let Some(path) = &state.storage_path {
            write_rules_file(path, &next)?;
        }
        state.rules = next;
        Ok(changed)
    }

    #[allow(dead_code)]
    pub fn clear_for_tests(&self) {
        *self.inner.lock() = DynamicTriggerRegistryState::default();
    }
}

pub fn global_registry() -> &'static DynamicTriggerRegistry {
    static CELL: OnceCell<DynamicTriggerRegistry> = OnceCell::new();
    CELL.get_or_init(DynamicTriggerRegistry::new)
}

pub fn set_dynamic_trigger_poll_interval_secs(secs: u64) {
    CONFIGURED_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS.store(secs.max(1), Ordering::Relaxed);
}

pub fn dynamic_trigger_poll_interval_secs() -> u64 {
    CONFIGURED_DYNAMIC_TRIGGER_POLL_INTERVAL_SECS.load(Ordering::Relaxed)
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

fn read_rules_file(path: &Path) -> Result<Vec<DynamicTriggerRule>, DynamicTriggerStorageError> {
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

fn write_rules_file(
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
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("triggers/dynamic");
