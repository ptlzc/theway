//! Project-level "last-set" memory for subagent model/thinking overrides.
//!
//! One [`SubagentSettingsStore`] per project directory reads and writes
//! `<project>/.pi/subagent-settings.json` (the path rule lives in
//! `theway-contract`). The store is shared across sessions of one project
//! through [`SubagentSettingsRegistry`], so the last explicitly set
//! `provider` / `model` / `thinking` values apply to later `dag_plan` node
//! definitions and `subagent` tool calls that do not override them.
//!
//! Merge policy ([`merge_run_settings`]):
//! - `provider` + `model` form one unit: when either is explicitly present in
//!   the caller's input (an empty string counts as present and clears the
//!   pair), the pair comes from the input verbatim; otherwise the remembered
//!   pair is inherited.
//! - `thinking` is independent: an explicit value (empty string = clear) wins;
//!   otherwise the remembered value is inherited.
//!
//! Memory writes are best-effort: failures are logged, never surfaced to the
//! caller. Writes happen only after a plan is accepted / a model resolves, so
//! rejected input never poisons the memory. A remembered `provider` without a
//! `model` can never launch and is never written.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use theway_contract::subagent_settings::{
    SubagentRunSettings, SubagentSettingsFile, subagent_settings_path_for_project,
};
use theway_core::multiagent::graph::types::DagNodeDef;

/// One project's settings store. Clones share one per-path async lock, so
/// concurrent sessions of the same project serialize their reads and writes.
#[derive(Clone)]
pub struct SubagentSettingsStore {
    path: PathBuf,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl SubagentSettingsStore {
    /// Create the store for `project_dir` (the project root; the settings file
    /// lives under its `.pi` directory). The project path is canonicalized so
    /// every spelling of the same directory shares one store instance.
    pub fn new(project_dir: &Path) -> Self {
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        Self {
            path: subagent_settings_path_for_project(&canonical.join(".pi")),
            lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Load the settings file. A missing or unreadable file yields the
    /// default; a corrupt file is logged and treated as default (it is only
    /// replaced by the next successful save).
    async fn load_unlocked(&self) -> SubagentSettingsFile {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(file) => file,
                Err(e) => {
                    tracing::warn!(path = %self.path.display(), "subagent settings parse: {e}");
                    SubagentSettingsFile::default()
                }
            },
            Err(_) => SubagentSettingsFile::default(),
        }
    }

    /// Atomically write the settings file (temp file + rename); creates the
    /// `.pi` directory when missing.
    async fn save_unlocked(&self, file: &SubagentSettingsFile) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| format!("settings path has no parent: {}", self.path.display()))?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(file).map_err(|e| e.to_string())?;
        tokio::fs::write(&tmp, &data)
            .await
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        tokio::fs::rename(&tmp, &self.path)
            .await
            .map_err(|e| format!("rename {} -> {}: {e}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    /// Merge remembered defaults into node definitions. Returns the
    /// `(node id, effective settings)` entries to upsert with
    /// [`Self::remember_nodes`] once the plan is accepted.
    pub async fn merge_nodes(
        &self,
        nodes: &mut [DagNodeDef],
    ) -> Vec<(String, SubagentRunSettings)> {
        let _guard = self.lock.lock().await;
        let file = self.load_unlocked().await;
        merge_nodes_settings(nodes, &file.nodes)
    }

    /// Upsert the entries produced by [`Self::merge_nodes`]; an empty entry
    /// removes the remembered record. Skips the write when nothing changed.
    pub async fn remember_nodes(&self, updates: Vec<(String, SubagentRunSettings)>) {
        if updates.is_empty() {
            return;
        }
        let _guard = self.lock.lock().await;
        let mut file = self.load_unlocked().await;
        let mut changed = false;
        for (id, entry) in updates {
            if file.nodes.get(&id) == Some(&entry) {
                continue;
            }
            if entry.is_empty() {
                file.nodes.remove(&id);
            } else {
                file.nodes.insert(id, entry);
            }
            changed = true;
        }
        if changed {
            self.persist(&file).await;
        }
    }

    /// Effective override triple for one subagent spec: the caller's explicit
    /// values merged over the remembered record for `name`.
    pub async fn merge_agent(
        &self,
        name: &str,
        provider: Option<&str>,
        model: Option<&str>,
        thinking: Option<&str>,
    ) -> SubagentRunSettings {
        let _guard = self.lock.lock().await;
        let file = self.load_unlocked().await;
        let remembered = file.agents.get(name).cloned().unwrap_or_default();
        merge_run_settings(provider, model, thinking, &remembered)
    }

    /// Upsert one agent record; an empty entry removes it. Skips the write
    /// when nothing changed.
    pub async fn remember_agent(&self, name: &str, entry: SubagentRunSettings) {
        let _guard = self.lock.lock().await;
        let mut file = self.load_unlocked().await;
        if file.agents.get(name) == Some(&entry) {
            return;
        }
        if entry.is_empty() {
            file.agents.remove(name);
        } else {
            file.agents.insert(name.to_string(), entry);
        }
        self.persist(&file).await;
    }

    async fn persist(&self, file: &SubagentSettingsFile) {
        if let Err(e) = self.save_unlocked(file).await {
            tracing::warn!(path = %self.path.display(), "subagent settings save: {e}");
        }
    }
}

/// Process-scoped registry of per-project settings stores: one shared store
/// per project directory, injected into every session's tool set.
#[derive(Clone, Default)]
pub struct SubagentSettingsRegistry {
    stores: Arc<Mutex<HashMap<PathBuf, Arc<SubagentSettingsStore>>>>,
}

impl SubagentSettingsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The shared store for `project_dir` (canonicalized).
    pub fn store_for(&self, project_dir: &Path) -> Arc<SubagentSettingsStore> {
        let canonical =
            std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());
        self.stores
            .lock()
            .entry(canonical)
            .or_insert_with(|| Arc::new(SubagentSettingsStore::new(project_dir)))
            .clone()
    }
}

fn opt_or_none(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.is_empty()).map(str::to_string)
}

/// Merge one explicit override triple with a remembered record (see the
/// module docs for the pair/thinking policy).
pub fn merge_run_settings(
    explicit_provider: Option<&str>,
    explicit_model: Option<&str>,
    explicit_thinking: Option<&str>,
    remembered: &SubagentRunSettings,
) -> SubagentRunSettings {
    let pair_explicit = explicit_provider.is_some() || explicit_model.is_some();
    let (provider, model) = if pair_explicit {
        (opt_or_none(explicit_provider), opt_or_none(explicit_model))
    } else {
        (remembered.provider.clone(), remembered.model.clone())
    };
    let thinking = match explicit_thinking {
        Some(raw) => opt_or_none(Some(raw)),
        None => remembered.thinking.clone(),
    };
    SubagentRunSettings {
        provider,
        model,
        thinking,
    }
}

/// Apply remembered defaults to every node (keyed by node id). Returns the
/// upsert entries for [`SubagentSettingsStore::remember_nodes`]: only records
/// that changed, and never a `provider`-without-`model` pair (it cannot
/// launch, so it is applied for this run but not remembered).
pub fn merge_nodes_settings(
    nodes: &mut [DagNodeDef],
    remembered: &BTreeMap<String, SubagentRunSettings>,
) -> Vec<(String, SubagentRunSettings)> {
    let mut updates = Vec::new();
    for node in nodes {
        let prior = remembered.get(&node.id).cloned().unwrap_or_default();
        let effective = merge_run_settings(
            node.provider.as_deref(),
            node.model.as_deref(),
            node.thinking.as_deref(),
            &prior,
        );
        let valid_pair = !(effective.provider.is_some() && effective.model.is_none());
        if valid_pair && effective != prior {
            updates.push((node.id.clone(), effective.clone()));
        }
        node.provider = effective.provider.clone();
        node.model = effective.model.clone();
        node.thinking = effective.thinking.clone();
    }
    updates
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("subagent_settings");
