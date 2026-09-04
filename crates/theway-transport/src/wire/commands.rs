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
        session_id: String,
        text: String,
        images: Vec<WirePromptImage>,
        /// true = stop the current turn and run this message now (INTERRUPT);
        /// false = queue after the current turn (QUEUE, default).
        interrupt: bool,
    },
    TriggerRuleNow {
        id: String,
    },
    Abort {
        session_id: String,
    },
    ResolveControlPlane {
        session_id: String,
        approve: bool,
    },
    SetModel {
        session_id: String,
        spec: String,
        response: tokio::sync::oneshot::Sender<bool>,
    },
    /// Set the active thinking level (mirrors the `/thinking` slash command;
    /// typed RPC so the client can confirm via snapshot before persisting).
    SetThinking {
        session_id: String,
        level: String,
        response: tokio::sync::oneshot::Sender<bool>,
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
    /// Invoke one daemon-owned extension command on the serialized runtime.
    InvokeExtensionCommand {
        name: String,
        arguments: serde_json::Value,
        has_interactive_client: bool,
        response: tokio::sync::oneshot::Sender<Result<WireExtensionCommandOutcome, String>>,
    },
    /// Re-discover and atomically reload runtime extensions. Active work may
    /// leave the request pending until its quiescent settlement boundary.
    ReloadExtensions {
        cancel_active: bool,
        response: tokio::sync::oneshot::Sender<Result<WireExtensionReloadResult, String>>,
    },
    /// Persist a project or exact-package trust decision, then reload.
    DecideExtensionTrust {
        request: WireExtensionTrustRequest,
        response: tokio::sync::oneshot::Sender<Result<WireExtensionTrustResult, String>>,
    },
    ActivateSession {
        request: WireActivateSessionRequest,
        response: tokio::sync::oneshot::Sender<Result<WireActivateSessionResponse, WireRpcError>>,
    },
    SetCredential {
        request: WireSetCredentialRequest,
        response: tokio::sync::oneshot::Sender<Result<(), WireRpcError>>,
    },
    ClearCredential {
        request: WireClearCredentialRequest,
        response: tokio::sync::oneshot::Sender<Result<(), WireRpcError>>,
    },
    /// A session was deleted through a transport RPC after the repo delete
    /// already succeeded. The event loop drops the deleted session's runtime
    /// (parked), or — when it was the ACTIVE session — swaps the active
    /// runtime to the most recent remaining session so later attaches never
    /// land on a deleted session.
    SessionDeleted {
        id: String,
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
    /// Full thinking level ("off" | "minimal" | "low" | "medium" | "high" |
    /// "xhigh") — the persisted last-choice default. Finer-grained than the
    /// legacy `thinking` toggle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    /// Enabled builtin skill names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_skills: Vec<String>,
    /// Controller-scanned skills (issue #95): the TUI owns local skill
    /// discovery and provisions the daemon with the scanned catalog —
    /// name/description/body included, so the daemon never reads skill files
    /// in a controller-provisioned session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<WireProvisionedSkill>,
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

/// One controller-scanned skill (issue #95): the TUI walks the local skill
/// roots and provisions the daemon with the full catalog so a
/// controller-provisioned session never needs the daemon to read skill files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WireProvisionedSkill {
    /// Canonical skill name (matches the daemon-side `Skill.name` rules).
    pub name: String,
    /// Free-form description shown in the catalog / system prompt.
    pub description: String,
    /// Markdown body without the YAML frontmatter.
    pub content: String,
    /// Absolute path to the source `SKILL.md` (or root-level `.md`).
    pub file_path: String,
    /// Discovery layer: `"user"` or `"project"` (mirrors `SkillSource`).
    pub source: String,
    /// When true the model must not auto-invoke this skill.
    pub disable_model_invocation: bool,
}

impl WireDaemonConfig {
    pub const FIELDS: [&'static str; 12] = [
        "provider",
        "model",
        "base_url",
        "thinking",
        "thinking_level",
        "builtin_skills",
        "skills",
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
                "thinking_level" => self.thinking_level.take().is_some(),
                "builtin_skills" => !std::mem::take(&mut self.builtin_skills).is_empty(),
                "skills" => !std::mem::take(&mut self.skills).is_empty(),
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
        if let Some(level) = patch.thinking_level.clone() {
            self.thinking_level = Some(level);
            touched += 1;
        }
        if !patch.builtin_skills.is_empty() {
            self.builtin_skills = patch.builtin_skills.clone();
            touched += 1;
        }
        if !patch.skills.is_empty() {
            self.skills = patch.skills.clone();
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

