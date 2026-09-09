//! Project-level subagent run settings: the last-set model/thinking overrides
//! remembered per DAG node id and per subagent spec name.
//!
//! One JSON file per project lives at `<project>/.pi/subagent-settings.json`.
//! It is session-independent (unlike the DAG run snapshots), so every session
//! of a project shares the same last-set values. The daemon merges these
//! remembered values into new `dag_plan` node definitions and `subagent` tool
//! calls whenever the caller does not pass an explicit override.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Model/thinking overrides remembered for one DAG node id or one subagent
/// spec name. Every field is optional; `None` means "no override" for that
/// field.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRunSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl SubagentRunSettings {
    /// True when the record carries no override at all; such records are not
    /// kept in the settings file.
    pub fn is_empty(&self) -> bool {
        self.provider.is_none() && self.model.is_none() && self.thinking.is_none()
    }
}

/// The settings file: last-set overrides keyed by DAG node id (`nodes`) and
/// subagent spec name (`agents`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSettingsFile {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub nodes: BTreeMap<String, SubagentRunSettings>,
    #[serde(default)]
    pub agents: BTreeMap<String, SubagentRunSettings>,
}

impl Default for SubagentSettingsFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            nodes: BTreeMap::new(),
            agents: BTreeMap::new(),
        }
    }
}

fn default_version() -> u32 {
    1
}

/// Path of the project-level settings file under a project's `.pi` directory.
pub fn subagent_settings_path_for_project(pi_dir: &Path) -> PathBuf {
    pi_dir.join("subagent-settings.json")
}
