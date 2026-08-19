//! Wire protocol model shared by the `--http` (axum) and `--grpc` (tonic) transport
//! servers: the command enum both event loops consume and the status payload both
//! serialize. Decoupled from the terminal UI — the servers live in the `transport`
//! module, and the serialized event loop is driven by the daemon kernel through the
//! [`crate::host::TransportHost`] surface. The proto codecs that map these models onto
//! the generated gRPC types live in `transport::proto` as well.

use serde::{Deserialize, Serialize};

/// One model entry in the picker group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
}

/// Filtered + grouped catalog with live credential detection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderGroup {
    pub provider: String,
    pub has_credential: bool,
    pub models: Vec<ModelEntry>,
}

#[derive(Clone)]
pub struct WebOptions {
    pub host: String,
    pub port: u16,
    /// Called with the actual bound address after the listener is up (used to
    /// publish the port when `port: 0` requested a random one).
    pub on_listen: Option<std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>>,
}

#[derive(Debug)]
pub enum WireCommand {
    Submit {
        text: String,
        images: Vec<WirePromptImage>,
        /// true = stop the current turn and run this message now (INTERRUPT);
        /// false = queue after the current turn (QUEUE, default).
        interrupt: bool,
    },
    TriggerRuleNow {
        id: String,
    },
    Abort,
    ResolveControlPlane {
        approve: bool,
    },
    SetModel {
        spec: String,
    },
    /// session-resource-model: switch the runtime to another session (resume semantics).
    /// `CreateSession`'s "make current" path also flows through this command — creating the
    /// session is a sync `SessionOps` call, becoming current goes through the serialized
    /// event loop.
    SwitchSession {
        id: String,
    },
    /// dynamic skills dirs (issue #68): replace the extra skill directories and
    /// hot-reload skills from disk. The event loop applies this authoritatively;
    /// the gRPC server optimistically updates the shared path context first.
    SetSkillDirs {
        dirs: Vec<String>,
    },
    /// Settings/config push (issue #72): apply a partial daemon configuration
    /// update. The event loop validates and applies it before updating the
    /// shared configuration view.
    Configure {
        config: WireDaemonConfig,
    },
}

/// Daemon configuration snapshot / partial update (issue #72) — the serde twin
/// of `settings.proto` `DaemonConfig`, shared by the JSON-RPC (`get_config` /
/// `set_config` / `configure`) and gRPC (`SettingsService`) surfaces.
///
/// Update semantics mirror the proto contract: a `Some` optional field
/// replaces the daemon's current value, `None` keeps it; repeated fields apply
/// only when non-empty. [`Self::clear_fields`] carries explicit clears.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireDaemonConfig {
    /// Model selection: provider name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model selection: model id within the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Custom provider endpoint URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Extended-thinking toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    /// Enabled builtin skill names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_skills: Vec<String>,
    /// Extra skill search directories (mirrors `WirePathContext::skills_dirs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills_dirs: Vec<String>,
    /// Trigger poll interval in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_poll_secs: Option<u64>,
    /// TUI feed scrollback cap (`[tui] max_feed_lines`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui_max_feed_lines: Option<u64>,
    /// Controller ToolService endpoint (`host:port`) for forwarded file/process
    /// operations (issue #77). `None` = daemon should not forward (or no
    /// controller tool server is available).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_service_addr: Option<String>,
    /// Controller StorageService endpoint (`host:port`) for controller-backed
    /// runtime storage (issue #85). `None` = daemon keeps using
    /// `LocalRuntimeStorage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_service_addr: Option<String>,
    /// Field names to clear before applying the values above. Snapshots never
    /// retain this patch-only field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clear_fields: Vec<String>,
}

impl WireDaemonConfig {
    pub const FIELDS: [&'static str; 10] = [
        "provider",
        "model",
        "base_url",
        "thinking",
        "builtin_skills",
        "skills_dirs",
        "trigger_poll_secs",
        "tui_max_feed_lines",
        "tool_service_addr",
        "storage_service_addr",
    ];

    pub fn clears(&self, field: &str) -> bool {
        self.clear_fields.iter().any(|candidate| candidate == field)
    }

    pub fn unknown_clear_fields(&self) -> Vec<&str> {
        self.clear_fields
            .iter()
            .map(String::as_str)
            .filter(|field| !Self::FIELDS.contains(field))
            .collect()
    }

    /// Apply a partial update: `Some` optional fields replace the current
    /// value, non-empty repeated fields replace the current list. Returns the
    /// number of config areas touched (for diagnostics).
    pub fn merge_from(&mut self, patch: &WireDaemonConfig) -> usize {
        let mut touched = 0;
        for field in &patch.clear_fields {
            let cleared = match field.as_str() {
                "provider" => self.provider.take().is_some(),
                "model" => self.model.take().is_some(),
                "base_url" => self.base_url.take().is_some(),
                "thinking" => self.thinking.take().is_some(),
                "builtin_skills" => !std::mem::take(&mut self.builtin_skills).is_empty(),
                "skills_dirs" => !std::mem::take(&mut self.skills_dirs).is_empty(),
                "trigger_poll_secs" => self.trigger_poll_secs.take().is_some(),
                "tui_max_feed_lines" => self.tui_max_feed_lines.take().is_some(),
                "tool_service_addr" => self.tool_service_addr.take().is_some(),
                "storage_service_addr" => self.storage_service_addr.take().is_some(),
                _ => false,
            };
            touched += usize::from(cleared);
        }
        if let Some(provider) = patch.provider.clone() {
            self.provider = Some(provider);
            touched += 1;
        }
        if let Some(model) = patch.model.clone() {
            self.model = Some(model);
            touched += 1;
        }
        if let Some(base_url) = patch.base_url.clone() {
            self.base_url = Some(base_url);
            touched += 1;
        }
        if let Some(thinking) = patch.thinking {
            self.thinking = Some(thinking);
            touched += 1;
        }
        if !patch.builtin_skills.is_empty() {
            self.builtin_skills = patch.builtin_skills.clone();
            touched += 1;
        }
        if !patch.skills_dirs.is_empty() {
            self.skills_dirs = patch.skills_dirs.clone();
            touched += 1;
        }
        if let Some(secs) = patch.trigger_poll_secs {
            self.trigger_poll_secs = Some(secs);
            touched += 1;
        }
        if let Some(lines) = patch.tui_max_feed_lines {
            self.tui_max_feed_lines = Some(lines);
            touched += 1;
        }
        if let Some(addr) = patch.tool_service_addr.clone() {
            self.tool_service_addr = Some(addr);
            touched += 1;
        }
        if let Some(addr) = patch.storage_service_addr.clone() {
            self.storage_service_addr = Some(addr);
            touched += 1;
        }
        touched
    }
}

/// Daemon path context (issue #68): startup-fixed home / base / work_dir plus
/// the current skill search directories. Served by `GetPathContext`;
/// `skills_dirs` is the only mutable part (via `SetSkillDirs`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WirePathContext {
    pub home: String,
    pub base: String,
    pub work_dir: String,
    pub skills_dirs: Vec<String>,
}

/// session-resource-model: one session as a managed resource (mirrors
/// `crates/theway-transport/proto/session.proto` SessionSummary). Produced by the app-side SessionOps
/// by the host's [`crate::transport::SessionOps`] implementation; served
/// verbatim on JSON and mapped onto the protobuf message by this crate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    pub model: String,
    pub created_at: String,
    pub last_activity_at: i64,
    pub graph_count: u32,
    pub active_graph_count: u32,
    pub busy: bool,
    pub preview: Option<String>,
}

/// graph mode: one DAG run (mirrors `crates/theway-transport/proto/graph_engine.proto` DagRunSnapshot; task text is
/// deliberately excluded from the wire model — full text goes through GetNodeOutput).
#[derive(Clone, Debug, Serialize)]
pub struct WireDagRunSnapshot {
    pub id: String,
    pub name: String,
    /// "dag" | "goal" — goal runs are single-node self-loops (condition-terminated).
    pub kind: String,
    pub status: String,
    pub fail_fast: bool,
    pub max_concurrency: usize,
    pub direction: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub nodes: Vec<WireDagNodeSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireDagNodeSnapshot {
    pub id: String,
    pub agent: String,
    pub status: String,
    pub depends_on: Vec<String>,
    pub job_id: Option<String>,
    pub attempt: u32,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub error: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub result: Option<WireNodeResultSnapshot>,
    pub output_tail: Option<String>,
    pub live_preview: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireNodeResultSnapshot {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
}

/// Node transcript/output returned by the transport-side
/// [`crate::transport::JobOps`] seam.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WireNodeOutput {
    /// `None` means no live or retained job exists for this run/node pair.
    pub output: Option<String>,
    pub truncated: bool,
    pub messages: Option<Vec<serde_json::Value>>,
    pub messages_truncated: bool,
}

/// Portable graph checkpoint returned by the transport-side
/// [`crate::transport::GraphOps`] seam.
#[derive(Clone, Debug, PartialEq)]
pub struct WireGraphCheckpoint {
    pub kind: WireGraphKind,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireGraphKind {
    Dag,
    Goal,
}

/// Subagent event already projected into protocol-owned values.
#[derive(Clone, Debug, PartialEq)]
pub enum WireAgentEvent {
    Started {
        id: String,
        agent: String,
        source: String,
        run_id: Option<String>,
        node_id: Option<String>,
    },
    Output {
        id: String,
        chunk: String,
    },
    Metrics {
        id: String,
        tps: Option<f64>,
        cps: Option<f64>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
        turn: u32,
    },
    Completed {
        id: String,
        status: String,
        error: Option<String>,
        chars: u64,
        tokens_in: u64,
        tokens_out: u64,
        tools_called: u64,
    },
}

/// DAG event already projected into protocol-owned string statuses.
#[derive(Clone, Debug, PartialEq)]
pub enum WireDagEvent {
    NodeStatus {
        run_id: String,
        session_id: String,
        node_id: String,
        status: String,
        error: Option<String>,
    },
    RunStatus {
        run_id: String,
        session_id: String,
        status: String,
        error: Option<String>,
    },
}

/// Graph mode: one subagent job projected by the host's job adapter (mirrors
/// `crates/theway-transport/proto/graph_engine.proto` SubagentJobSnapshot).
#[derive(Clone, Debug, Serialize)]
pub struct WireAgentJobSnapshot {
    pub id: String,
    pub agent: String,
    pub source: String,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub status: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<u64>,
    pub attempt: u32,
    pub total_attempts: u32,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub error: Option<String>,
    pub output_tail: Option<String>,
    pub live_preview: Option<String>,
    pub tps: Option<f64>,
    pub cps: Option<f64>,
    pub chars: Option<u64>,
    pub tools_called: Option<u64>,
    pub turn: Option<u32>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct WireContextUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    pub context_window: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireStatus {
    pub session_id: String,
    pub model: String,
    pub model_catalog: Vec<ProviderGroup>,
    pub cwd: String,
    pub busy: bool,
    pub queued_count: usize,
    pub latest_trigger_poll: Option<crate::feed::TriggerPollStatus>,
    pub goal: Option<WireGoalSnapshot>,
    pub control_plane_prompt: Option<WireControlPlanePromptSnapshot>,
    pub sidebar: WireSidebarSnapshot,
    pub feed_blocks: Vec<crate::feed::WireFeedBlock>,
    /// Required consumer block count before applying
    /// [`Self::feed_block_patches`]. Zero with no patches is a full frame.
    #[serde(default)]
    pub feed_blocks_base: u64,
    /// Incremental block appends/replacements for gRPC stream consumers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feed_block_patches: Vec<WireFeedBlockPatch>,
    pub feed_lines: Vec<String>,
    /// Absolute index of `feed_lines[0]` in a gRPC incremental stream frame.
    /// Authoritative `WireStatus` snapshots keep this at zero and carry every
    /// row; per-client stream projection applies the non-zero cursor.
    #[serde(default)]
    pub feed_lines_base: u64,
    pub dags: Vec<WireDagRunSnapshot>,
    pub subagents: Vec<WireAgentJobSnapshot>,
    /// Running token usage + the active model's context window, published by
    /// the daemon for the TUI prompt chrome (context-usage indicator).
    #[serde(default)]
    pub usage: WireContextUsage,
    /// TUI display settings resolved by the daemon from `config.toml`
    /// (`[tui] max_feed_lines`); `None` → the TUI built-in default applies.
    pub tui_max_feed_lines: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WireFeedBlockPatch {
    pub index: u64,
    pub block: crate::feed::WireFeedBlock,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireGoalSnapshot {
    pub condition: String,
    pub status: String,
    pub iterations: u32,
    pub last_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireControlPlanePromptSnapshot {
    pub tool_name: String,
    pub label: String,
    pub reason: String,
    pub args_hash: String,
    pub payload: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireSidebarSnapshot {
    pub inbox_new: usize,
    pub skills: WireSkillsSnapshot,
    pub triggers: WireTriggersSnapshot,
    pub cron: WireCronSnapshot,
    pub mcp: WireMcpSnapshot,
    pub tools: WireToolsSnapshot,
    pub hooks: Vec<String>,
    pub runtime: Vec<String>,
    /// Slash-prefixed file-command names discovered from `.agents/commands`
    /// and `.claude/commands` (claude-code format, issue #37).
    #[serde(default)]
    pub commands: Vec<String>,
    /// Runtime-reload epoch (issue #50): the daemon bumps this after a
    /// successful `reload` tool call; clients cache it and re-read local
    /// resources (e.g. `~/.theway/theme.toml`) when it changes. Serde
    /// default keeps older snapshots decodable.
    #[serde(default)]
    pub runtime_revision: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireSkillsSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub builtin: usize,
    pub user: usize,
    pub project: usize,
    pub items: Vec<WireSkillSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireSkillSnapshot {
    pub name: String,
    pub source: String,
    pub file_path: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireTriggersSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub rules: Vec<WireTriggerRuleSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireTriggerRuleSnapshot {
    pub id: String,
    pub full_id: String,
    pub enabled: bool,
    pub mode: String,
    pub condition: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireCronSnapshot {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub jobs: Vec<WireCronJobSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireCronJobSnapshot {
    pub id: String,
    pub enabled: bool,
    pub schedule: String,
    pub action: String,
    pub skipped_overlap_count: u64,
    pub last_error: Option<String>,
}

// ──────────────────────────────────────────────────────────────────────────
// Runtime state externalization (issue #84): serde twins of `state.proto`
// `StorageService` messages. These are the portable wire forms used by the
// JSON-RPC state methods, the gRPC `StorageService` codecs (`crate::state`),
// and the [`crate::transport::StorageOps`] handler seam the daemon implements
// behind the `RuntimeStorage` boundary.
// ──────────────────────────────────────────────────────────────────────────

/// One persisted DAG run: `snapshot` is the portable JSON `PersistedRun`
/// shape used by GraphEngineService checkpoint/restore.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireStoredDagRun {
    pub session_id: String,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveDagRunRequest {
    pub session_id: String,
    pub run_id: String,
    pub snapshot: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveDagRunResult {
    pub saved: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadDagRunsRequest {
    pub session_id: String,
    /// `None` = load every stored run for the session; `Some` = load one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadDagRunsResult {
    pub runs: Vec<WireStoredDagRun>,
}

/// Session-scoped dynamic trigger rule in portable storage form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireStoredTriggerRule {
    pub id: String,
    pub condition: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default = "default_fire_once")]
    pub fire_once: bool,
    /// RFC3339, when the rule has fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<String>,
    #[serde(default)]
    pub promote_to_chat: bool,
    /// RFC3339.
    pub created_at: String,
}

impl Default for WireStoredTriggerRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            condition: String::new(),
            action: String::new(),
            enabled: false,
            fire_once: true,
            fired_at: None,
            promote_to_chat: false,
            created_at: String::new(),
        }
    }
}

fn default_fire_once() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveTriggerRulesRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<WireStoredTriggerRule>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveTriggerRulesResult {
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadTriggerRulesRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadTriggerRulesResult {
    pub rules: Vec<WireStoredTriggerRule>,
}

/// Session-scoped cron job in portable storage form.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireStoredCronJob {
    pub id: String,
    /// Standard 5-field cron expression.
    pub schedule: String,
    pub action: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_trace_id: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_due_at: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<String>,
    /// RFC3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub skipped_overlap_count: u64,
    #[serde(default)]
    pub stateful: bool,
    /// RFC3339.
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveCronJobsRequest {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<WireStoredCronJob>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireSaveCronJobsResult {
    pub count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadCronJobsRequest {
    pub session_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireLoadCronJobsResult {
    pub jobs: Vec<WireStoredCronJob>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireMcpSnapshot {
    pub servers: usize,
    pub tools: usize,
    pub notification_hooks: usize,
    pub server_names: Vec<String>,
    pub tool_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WireToolsSnapshot {
    pub total: usize,
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WirePromptImage {
    pub data: String,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetModelRequest {
    pub model: String,
}

// ──────────────────────────────────────────────────────────────────────────
// Tool-operation domain (issue #75): the serde twin of `tools.proto`, shared
// by the JSON-RPC surface (`read_file` / `write_file` / … / `skill_install`),
// the gRPC `ToolService` codecs (`crate::tools`), and the [`crate::transport::
// ToolOps`] handler seam the daemon implements.
// ──────────────────────────────────────────────────────────────────────────

/// Tool-operation failure (issue #75). The gRPC surface maps the variants onto
/// tonic status codes (`not_found` / `invalid_argument` / `internal`); the
/// JSON-RPC surface maps them onto `-32004` / `-32602` / `-32000`.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error(transparent)]
    Other(anyhow::Error),
}

impl ToolError {
    /// Build an [`ToolError::Other`] from any displayable error.
    pub fn other(message: impl std::fmt::Display) -> Self {
        ToolError::Other(anyhow::anyhow!("{message}"))
    }
}

/// Read a file as UTF-8 text with line pagination (1-based `offset`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolReadRequest {
    pub path: String,
    /// First line to return (1-based). `None` = from the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Maximum number of lines to return. `None` = the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolReadResult {
    pub content: String,
    pub total_lines: u64,
    /// More lines follow beyond the returned window.
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolWriteRequest {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolWriteResult {
    pub bytes_written: u64,
}

/// Search-and-replace edit: replace `old_string` with `new_string`, optionally
/// restricted to a 1-based inclusive line range.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolEditRequest {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_end: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolEditResult {
    pub replacements: u32,
}

/// Run a shell command line through the daemon's shell.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolExecRequest {
    pub command: String,
    /// Working directory. `None` = the daemon's work dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Wall-clock timeout in milliseconds. `None` = executor default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// One streaming exec frame: output chunks (interleaved stdout/stderr), then a
/// terminal exit frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireToolExecFrame {
    Output {
        text: String,
    },
    Exit {
        code: i32,
        timed_out: bool,
        duration_ms: u64,
    },
}

/// Unary collect of an exec stream — the shape the JSON-RPC surface returns
/// (request/response only; gRPC streams the frames individually).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolExecResult {
    pub output: String,
    pub code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolListDirRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolDirEntry {
    pub name: String,
    /// "file" | "dir" | "symlink" | "other"
    pub kind: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolListDirResult {
    pub entries: Vec<WireToolDirEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolGrepRequest {
    /// Regular expression.
    pub pattern: String,
    /// Search root. `None` = the daemon's work dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Filename glob filter (e.g. `*.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glob_filter: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
    /// "content" (default) | "files_with_matches" | "count".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolGrepMatch {
    pub path: String,
    pub line_number: u64,
    pub line: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolGrepFileCount {
    pub path: String,
    pub count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolGrepResult {
    /// output_mode "content".
    pub matches: Vec<WireToolGrepMatch>,
    /// output_mode "files_with_matches".
    pub files: Vec<String>,
    /// output_mode "count".
    pub counts: Vec<WireToolGrepFileCount>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolFindRequest {
    /// Filename glob (e.g. `*.proto`).
    pub pattern: String,
    /// Search root. `None` = the daemon's work dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolFindResult {
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemorySaveRequest {
    pub name: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Memory type tag ("user" | "preference" | …), free-form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemorySaveResult {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryListRequest {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    pub path: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryListResult {
    pub entries: Vec<WireToolMemoryEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryReadRequest {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryReadResult {
    pub name: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryForgetRequest {
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolMemoryForgetResult {
    pub removed: bool,
}

/// Skill source for `skill_install`: https URL, local path, or inline content
/// (same three sources as the `install_skill` agent tool).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireToolSkillSource {
    Url(String),
    Path(String),
    Content(String),
}

/// Two-phase install (same safety model as the `install_skill` agent tool):
/// without `confirm` the call is a read-only preview and installs nothing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireToolSkillInstallRequest {
    pub source: WireToolSkillSource,
    #[serde(default)]
    pub confirm: bool,
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireToolSkillInstallResult {
    pub name: String,
    pub target_path: String,
    /// false = preview only (`confirm` was not set); true = installed.
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub size: u64,
    pub existing: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
