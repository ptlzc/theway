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
