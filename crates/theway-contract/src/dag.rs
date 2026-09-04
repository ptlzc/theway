//! Engine-independent persisted DAG snapshot models and path layout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Pending,
    Ready,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunKind {
    #[default]
    Dag,
    Goal,
}

impl RunKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Dag => "dag",
            Self::Goal => "goal",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Direction {
    #[serde(rename = "TD")]
    Td,
    #[serde(rename = "LR")]
    Lr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeResult {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedNode {
    pub id: String,
    pub agent: String,
    pub task: String,
    pub depends_on: Vec<String>,
    pub timeout: Option<u64>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    pub status: NodeStatus,
    pub attempt: u32,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub result: Option<NodeResult>,
    pub output: Option<String>,
    pub live_preview: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedRun {
    pub id: String,
    pub name: String,
    pub max_concurrency: usize,
    pub fail_fast: bool,
    pub direction: Direction,
    pub created_at: i64,
    pub session_id: Option<String>,
    #[serde(default)]
    pub kind: RunKind,
    pub nodes: Vec<PersistedNode>,
}

/// State database path under a project's `.pi` directory.
pub fn state_path_for_project(pi_dir: &Path, session_id: Option<&str>) -> PathBuf {
    match session_id {
        Some(id) => pi_dir.join(format!(
            "graph-engineering-state-{}.db",
            sanitize_session_id(id)
        )),
        None => pi_dir.join("graph-engineering-state.db"),
    }
}

fn sanitize_session_id(session_id: &str) -> String {
    let clean: String = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if clean.is_empty() {
        "default".to_string()
    } else {
        clean
    }
}
