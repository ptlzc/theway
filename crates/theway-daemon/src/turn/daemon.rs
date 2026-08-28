//! `TurnHost` — the headless transport host behind the `thewayd` binary.
//!
//! It implements [`theway_transport::host::TransportHost`] and coordinates one
//! active session with the gRPC/HTTP/MCP servers from `theway-transport`.
//!
//! Startup assembly (harness, session, trigger executor, listeners, capabilities)
//! lives in the `thewayd` binary; this module only owns the serialized transport
//! event loop and the state it drives.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

use theway_core::AgentMessage;
use theway_core::SkillSource;
use theway_core::multiagent::graph::types::DagEvent;

use super::feed::{self, Feed, FeedUpdate, Level, TriggerPollStatus};
use super::kernel::{QueuedTurn, ReplKernel, TurnState, poll_turn};
use crate::agent_session::RetrySettings;
use crate::bug_report;
use crate::commands::{self, CommandCtx, CommandOutcome, Registry};
use crate::control_plane_prompt::PendingControlPlanePrompt;
use crate::forwarding_tool_ops::ForwardingToolOps;
use crate::orchestration::DaemonServices;
use crate::paths::DaemonPaths;
use crate::runtime_storage::SessionRepository;
use crate::session_ops::SessionFactory;
use crate::tools::assembly::reload::ReloadRuntime;
use crate::transport_adapter::{
    CoreGraphOps, CoreJobOps, agent_event, dag_event, dag_run_snapshot, subagent_job_snapshot,
};
use theway_llm_provider::{ImageContent, Message, Usage};
use theway_transport::mentions;
use theway_transport::transport::SlashCompleter;
use theway_transport::transport::ToolOps;
use theway_transport::wire::*;
use theway_transport::{TransportEndpoints, TransportMode};

/// Model families surfaced through transport snapshots.
const SUPPORTED_APIS: [&str; 4] = ["openai-completions", "openai-responses", "anthropic", "ds4"];

/// Provider-grouped model catalog for transport snapshots.
fn model_catalog() -> Vec<ProviderGroup> {
    let mut groups: std::collections::BTreeMap<String, Vec<ModelEntry>> =
        std::collections::BTreeMap::new();
    for model in theway_llm_provider::list_models() {
        if !SUPPORTED_APIS.contains(&model.api.0.as_str()) {
            continue;
        }
        groups
            .entry(model.provider.0.clone())
            .or_default()
            .push(ModelEntry {
                id: model.id,
                name: model.name,
            });
    }
    groups
        .into_iter()
        .map(|(provider, mut models)| {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            ProviderGroup {
                has_credential: commands::model_credential_hint(&provider).is_none(),
                provider,
                models,
            }
        })
        .collect()
}

/// Runtime capabilities and inventory projected into transport snapshots.
#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeCapabilities {
    pub(crate) mcp_servers: usize,
    pub(crate) mcp_tools: usize,
    pub(crate) mcp_server_names: Vec<String>,
    pub(crate) mcp_tool_names: Vec<String>,
    pub(crate) tool_names: Vec<String>,
    pub(crate) mcp_notification_hooks: usize,
    pub(crate) hook_points: Vec<String>,
    pub(crate) trigger_features: Vec<String>,
}

/// Everything the daemon needs to run one session, assembled by the `thewayd` binary.
pub(crate) struct DaemonConfig {
    pub(crate) harness: Arc<theway_core::AgentHarness>,
    pub(crate) extension_host: Option<Arc<crate::ts_extensions::SessionPluginHost>>,
    pub(crate) trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
    pub(crate) retry: RetrySettings,
    pub(crate) registry: Registry,
    pub(crate) cwd: PathBuf,
    /// Startup-fixed home/base/work directory plus dynamically replaceable
    /// extra skill directories.
    pub(crate) paths: DaemonPaths,
    pub(crate) session_id: String,
    pub(crate) log_path: Option<PathBuf>,
    pub(crate) tool_count: usize,
    pub(crate) feed_rx: mpsc::UnboundedReceiver<FeedUpdate>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    pub(crate) feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    pub(crate) main_run_rx: mpsc::UnboundedReceiver<String>,
    pub(crate) control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<PendingControlPlanePrompt>>,
    pub(crate) dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    pub(crate) subagent_registry: theway_core::multiagent::jobs::SubagentJobRegistry,
    pub(crate) session_factory: SessionFactory,
    pub(crate) session_repo: Arc<dyn SessionRepository>,
    pub(crate) capabilities: RuntimeCapabilities,
    /// `[orchestrator] thinking_summary` settings; `None` → thinking stays raw.
    pub(crate) thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    /// In-memory startup settings (issue #73): defaults merged with the
    /// controller's initial settings payload — the values startup previously
    /// read from `config.toml` (feed-history cap, trigger poll interval,
    /// enabled builtin skills, …). Seeds the shared `GetConfig` view;
    /// runtime `Configure` updates merge into the view, not back into this
    /// startup snapshot.
    pub(crate) startup: crate::startup_config::StartupConfig,
    pub(crate) services: DaemonServices,
}

struct SessionRuntimeState {
    kernel: ReplKernel,
    id: String,
    cwd: PathBuf,
    log_path: Option<PathBuf>,
    tool_count: usize,
    retry: RetrySettings,
    factory: SessionFactory,
    repository: Arc<dyn SessionRepository>,
    busy: bool,
    queue: VecDeque<QueuedTurn>,
    cumulative_usage: WireContextUsage,
}

/// Per-session runtime registry.
///
/// The daemon keeps the active session in `TurnHost::session` for compatibility
/// with the existing transport snapshot path; all other live sessions are parked
/// here keyed by their explicit `session_id`. This lets commands address any
/// registered session without a global `SwitchSession` first.
struct SessionRegistry {
    sessions: HashMap<String, SessionRuntimeState>,
}

impl SessionRuntimeState {
    fn from_runtime(
        runtime: crate::orchestration::SessionRuntime,
        factory: SessionFactory,
        repository: Arc<dyn SessionRepository>,
        retry: crate::agent_session::RetrySettings,
        log_path: Option<PathBuf>,
    ) -> Self {
        let mut kernel = ReplKernel::new(runtime.harness, runtime.trigger_executor, retry.clone());
        kernel.set_extension_host(runtime.extension_host);
        let id = runtime.session_id;
        let cwd = runtime.cwd;
        let tool_count = runtime.tool_names.len();
        Self {
            kernel,
            id,
            cwd,
            log_path,
            tool_count,
            retry,
            factory,
            repository,
            busy: false,
            queue: VecDeque::new(),
            cumulative_usage: WireContextUsage::default(),
        }
    }
}

#[cfg(test)]
impl SessionRuntimeState {
    fn for_test(id: &str) -> Self {
        let storage = std::sync::Arc::new(theway_core::MemorySessionStorage::new());
        let session = theway_core::Session::new(storage as std::sync::Arc<dyn theway_core::SessionStorage>);
        let model = theway_llm_provider::Model {
            id: "faux".into(),
            name: "Faux".into(),
            api: theway_llm_provider::Api::from("faux"),
            provider: theway_llm_provider::Provider::from("faux"),
            base_url: String::new(),
            reasoning: false,
            thinking_level_map: None,
            input: vec![],
            cost: theway_llm_provider::ModelCost::default(),
            context_window: 0,
            max_tokens: 0,
            headers: None,
            compat: None,
        };
        let harness = std::sync::Arc::new(theway_core::AgentHarness::new(
            theway_core::AgentHarnessOptions::new(model, session),
        ));
        let trigger_executor = std::sync::Arc::new(
            crate::trigger_engine::execution::TriggerExecutor::new(
                harness.agent_arc(),
                harness.session().clone(),
                crate::trigger_engine::runtime::TriggerRuntimeConfig::default(),
                None,
                None,
                None,
                None,
                None,
                None,
            ),
        );
        let factory: SessionFactory = std::sync::Arc::new(|_| {
            Box::pin(async { anyhow::bail!("session factory unused in for_test") })
        });
        let repository: std::sync::Arc<dyn SessionRepository> = std::sync::Arc::new(
            theway_storage::sqlite_repo::SqliteSessionRepo::new(
                std::env::temp_dir().join("theway-test-session-registry"),
            ),
        );
        let mut kernel = ReplKernel::new(harness, trigger_executor, RetrySettings::default());
        kernel.set_extension_host(None);
        Self {
            kernel,
            id: id.to_string(),
            cwd: std::env::temp_dir().join("theway-test").join(id),
            log_path: None,
            tool_count: 0,
            retry: RetrySettings::default(),
            factory,
            repository,
            busy: false,
            queue: VecDeque::new(),
            cumulative_usage: WireContextUsage::default(),
        }
    }
}

impl SessionRegistry {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    fn insert(&mut self, runtime: SessionRuntimeState) {
        let id = runtime.id.clone();
        self.sessions.insert(id, runtime);
    }

    #[cfg(test)]
    fn get(&self, id: &str) -> Option<&SessionRuntimeState> {
        self.sessions.get(id)
    }

    fn get_mut(&mut self, id: &str) -> Option<&mut SessionRuntimeState> {
        self.sessions.get_mut(id)
    }

    fn contains(&self, id: &str) -> bool {
        self.sessions.contains_key(id)
    }

    fn remove(&mut self, id: &str) -> Option<SessionRuntimeState> {
        self.sessions.remove(id)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.sessions.len()
    }
}

struct AutomationRuntime {
    services: DaemonServices,
    reload: Arc<ReloadRuntime>,
    dag: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    subagents: theway_core::multiagent::jobs::SubagentJobRegistry,
}

struct RuntimeConfiguration {
    registry: Arc<Registry>,
    completer: SlashCompleter,
    cwd: PathBuf,
    /// Daemon path context (issue #68): home/base/work_dir are startup-fixed;
    /// the extra skill dirs are replaced at runtime by `SetSkillDirs` and the
    /// skill reload reads the fresh value through the shared `Arc`.
    paths: DaemonPaths,
    /// Shared wire path context (issue #68): served by `GetPathContext`;
    /// `skills_dirs` is the only field mutated at runtime (by the event loop
    /// when `SetSkillDirs` lands). Cloned into [`TransportEndpoints`] so the
    /// transport servers and this host observe one authoritative value.
    path_context: Arc<std::sync::RwLock<WirePathContext>>,
    /// Shared daemon configuration view (issue #72): served by `GetConfig`;
    /// seeded from the startup-resolved settings and merged by the event loop
    /// when `Configure` commands land. Cloned into [`TransportEndpoints`] so
    /// the transport servers and this host observe one authoritative value.
    config: Arc<std::sync::RwLock<WireDaemonConfig>>,
    /// Controller tool endpoint forwarder (issue #76): routes `ToolOps`
    /// calls to the connected controller's `ToolService` server.
    tool_ops: Arc<dyn ToolOps>,
    model_catalog: Vec<ProviderGroup>,
    /// Client feed-history preference exposed through the legacy wire field
    /// `tui_max_feed_lines`.
    feed_history_limit: Option<u64>,
    /// Optional publish handles used by activation to emit a coherent snapshot
    /// before replying to the client.
    latest: Option<Arc<Mutex<WireStatus>>>,
    snapshot_tx: Option<broadcast::Sender<WireStatusUpdate>>,
}

struct FeedProjectionState {
    feed: Feed,
    /// Incremental plain-text row cache behind full `feed_lines` snapshots.
    plain_lines_cache: theway_transport::feed::PlainLinesCache,
    /// Fingerprint of each block as last published on the patch stream.
    block_versions: Vec<u64>,
    /// In-place mutations since the last published snapshot. Appends and
    /// truncation are also detected from block counts.
    dirty_blocks: BTreeSet<usize>,
    latest_trigger_poll: Option<TriggerPollStatus>,
    latest_goal: Option<theway_core::multiagent::goal::GoalState>,
    thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    thinking_burst: super::thinking_summary::ThinkingBurst,
    control_plane_prompt: Option<PendingControlPlanePrompt>,
    capabilities: RuntimeCapabilities,
}

struct RuntimeEventInputs {
    feed_rx: Option<mpsc::UnboundedReceiver<FeedUpdate>>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    main_run_rx: Option<mpsc::UnboundedReceiver<String>>,
    control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<PendingControlPlanePrompt>>,
}

/// Headless transport host for `thewayd` (gRPC / HTTP / MCP).
///
/// The host is the serialized coordinator; state ownership remains explicit in
/// session, automation, runtime-configuration, feed-projection, and event-input
/// partitions.
pub(crate) struct TurnHost {
    session: SessionRuntimeState,
    sessions: SessionRegistry,
    automation: AutomationRuntime,
    runtime: RuntimeConfiguration,
    projection: FeedProjectionState,
    inputs: RuntimeEventInputs,
}

fn current_model_label(harness: &Arc<theway_core::AgentHarness>) -> String {
    let state = harness.agent().state();
    state
        .model
        .as_ref()
        .map(|m| format!("{}:{}", m.provider.0, m.id))
        .unwrap_or_else(|| "no-model".to_string())
}

/// Resolve the active model's context window (tokens) from the provider
/// catalog. `0` when unknown so clients can omit percentage indicators.
pub(crate) fn context_window_for(label: &str) -> u64 {
    let Some((provider, id)) = label.split_once(':') else {
        return 0;
    };
    theway_llm_provider::list_models()
        .iter()
        .find(|m| m.provider.0 == provider && m.id == id)
        .map(|m| u64::from(m.context_window))
        .unwrap_or(0)
}

fn user_facing_run_error(error: &str) -> String {
    let Some(rest) = error.strip_prefix("no API key for provider: ") else {
        return error.to_string();
    };
    let provider = rest.split(';').next().unwrap_or(rest).trim();
    if provider.is_empty() {
        return error.to_string();
    }
    let vars = theway_llm_provider::env_api_keys::env_var_names(provider);
    let credential_hint = if vars.is_empty() {
        "configure a provider-specific credential".to_string()
    } else {
        format!("set {}", vars.join(" or "))
    };
    format!("no API key for provider: {provider} ({credential_hint})")
}

fn slash_commands(registry: &Registry) -> Vec<String> {
    let mut commands: Vec<String> = registry
        .commands()
        .iter()
        .flat_map(|c| {
            let mut names = vec![format!("/{}", c.name())];
            names.extend(c.aliases().iter().map(|a| format!("/{a}")));
            names
        })
        .collect();
    // Claude-code-format file commands join the completion surface (issue #37).
    commands.extend(registry.file_command_names());
    commands
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/runtime.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/commands.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/input.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/snapshot.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/queue.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/state.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/turn/daemon/extensions.rs"
));

#[async_trait(?Send)]
impl theway_transport::host::TransportHost for TurnHost {
    fn transport_endpoints(&mut self) -> TransportEndpoints {
        TurnHost::transport_endpoints(self)
    }

    async fn run_transport_loop(
        self: Box<Self>,
        mode: TransportMode,
        endpoints: TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        (*self)
            .run_transport_loop(mode, endpoints, server_task)
            .await
    }
}

/// Token usage of the most recent completed LLM turn: the last assistant
/// message's `usage` (input/output/cache/total) in the transcript. `None`
/// before the first assistant reply; the snapshot then reports zeroed usage.
fn last_turn_usage(messages: &[AgentMessage]) -> Option<Usage> {
    messages.iter().rev().find_map(|m| match m {
        AgentMessage::Llm(Message::Assistant(a)) => Some(a.usage.clone()),
        _ => None,
    })
}

fn wire_preview(text: &str) -> String {
    feed::truncate_chars(&bug_report::redact(text), 120)
}

fn prompt_display(text: &str, image_count: usize) -> String {
    if image_count == 0 {
        text.chars().take(60).collect()
    } else {
        format!(
            "{} [{} image(s)]",
            text.chars().take(48).collect::<String>(),
            image_count
        )
    }
}

fn wire_control_plane_prompt_snapshot(
    request: &theway_core::ControlPlanePromptRequest,
) -> WireControlPlanePromptSnapshot {
    let payload = serde_json::to_string_pretty(&request.payload)
        .unwrap_or_else(|_| request.payload.to_string());
    WireControlPlanePromptSnapshot {
        tool_name: wire_prompt_text(&request.tool_name, 80),
        label: wire_prompt_text(&request.label, 160),
        reason: wire_prompt_text(&request.reason, 180),
        args_hash: request.args_hash.chars().take(12).collect(),
        payload: wire_prompt_text(&payload, 800),
    }
}

fn wire_prompt_text(text: &str, cap: usize) -> String {
    feed::truncate_chars(&bug_report::redact(text), cap)
}

fn load_web_prompt_images(images: &[WirePromptImage]) -> Result<Vec<ImageContent>> {
    if images.len() > theway_transport::images::MAX_IMAGES_PER_MESSAGE {
        bail!(
            "{} images exceeds per-message cap of {}",
            images.len(),
            theway_transport::images::MAX_IMAGES_PER_MESSAGE
        );
    }
    let mut out = Vec::with_capacity(images.len());
    for (idx, image) in images.iter().enumerate() {
        let label = image
            .name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| format!("clipboard image `{name}`"))
            .unwrap_or_else(|| format!("clipboard image #{}", idx + 1));
        let data = image
            .data
            .rsplit_once(',')
            .map(|(_, data)| data)
            .unwrap_or(image.data.as_str());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .with_context(|| format!("decode {label}"))?;
        let image = theway_transport::images::load_bytes(&label, &bytes)?;
        out.push(ImageContent {
            data: image.data,
            mime_type: image.mime_type,
        });
    }
    Ok(out)
}

#[cfg(test)]
// Test files live in `tests/turn/daemon/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("turn/daemon");

// Signal coverage sends process-wide SIGINT/SIGTERM, so every transport-loop
// test must run under the same lock to prevent a signal reaching a peer test.
#[cfg(test)]
pub(crate) static TRANSPORT_LOOP_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

#[cfg(test)]
mod daemon_more_tests {
    //! Additional turn-host tests live in `tests/turn/daemon/more/` so the
    //! primary `tests/turn/daemon/mod.rs` bridge stays untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/more");
}

#[cfg(test)]
mod daemon_extra_tests {
    //! Extra turn-host tests live in `tests/turn/daemon/extra/` so the
    //! primary `tests/turn/daemon/mod.rs` bridge stays untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/extra");
}

#[cfg(test)]
mod daemon_coverage_tests {
    //! Additional turn-host coverage tests live in
    //! `tests/turn/daemon/coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/coverage");
}

#[cfg(test)]
mod daemon_line_coverage_tests {
    //! Additional turn-host line coverage lives in
    //! `tests/turn/daemon/line_coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/line_coverage");
}

#[cfg(test)]
mod daemon_final_coverage_tests {
    //! Final turn-host line coverage lives in
    //! `tests/turn/daemon/final_coverage/`; separate bridge to keep the other
    //! mirrored suites untouched.
    tests_bridge_macro::tests_bridge!("turn/daemon/final_coverage");
}
