//! `TurnHost` — the headless transport host behind the `thewayd` binary.
//!
//! Same turn semantics, snapshot surface and command handling as the TUI's `App`
//! (the two share [`super::kernel`] and [`super::feed`]), but with no terminal:
//! it implements [`theway_transport::host::TransportHost`] and is driven by the
//! gRPC/HTTP/MCP protocol servers from `theway-transport`.
//!
//! Startup assembly (harness, session, trigger executor, listeners, panel status)
//! lives in the `thewayd` binary; this module only owns the serialized transport
//! event loop and the state it drives.

use std::collections::{BTreeSet, VecDeque};
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
use crate::commands::{self, CommandCtx, CommandOutcome, Registry};
use crate::control_plane_prompt::UiControlPlanePrompt;
use crate::forwarding_tool_ops::ForwardingToolOps;
use crate::paths::DaemonPaths;
use crate::session_ops::{CurrentSessionState, SessionFactory};
use crate::tools::assembly::reload::{self, ReloadRuntime};
use crate::transport_adapter::{
    CoreGraphOps, CoreJobOps, agent_event, dag_event, dag_run_snapshot, subagent_job_snapshot,
};
use crate::triggers;
use crate::{SqliteSessionRepo, bug_report};
use theway_llm_provider::{ImageContent, Message, Usage};
use theway_transport::mentions;
use theway_transport::transport::SlashCompleter;
use theway_transport::transport::ToolOps;
use theway_transport::wire::*;
use theway_transport::{TransportEndpoints, TransportMode};

/// Model families surfaced in the web/grpc model picker (mirror of the TUI's).
const SUPPORTED_APIS: [&str; 4] = ["openai-completions", "openai-responses", "anthropic", "ds4"];

/// Provider-grouped model catalog for transport snapshots.
pub fn model_catalog() -> Vec<ProviderGroup> {
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

/// Sidebar/panel statistics snapshot (skills/triggers/cron/MCP/tools/hooks) — the
/// daemon-side twin of the TUI's `PanelStatus`.
#[derive(Clone, Debug, Default)]
pub struct PanelStatus {
    pub mcp_servers: usize,
    pub mcp_tools: usize,
    pub mcp_server_names: Vec<String>,
    pub mcp_tool_names: Vec<String>,
    pub tool_names: Vec<String>,
    pub mcp_notification_hooks: usize,
    pub hook_points: Vec<String>,
    pub trigger_features: Vec<String>,
}

/// Everything the daemon needs to run one session, assembled by the `thewayd` binary.
pub struct DaemonConfig {
    pub harness: Arc<theway_core::AgentHarness>,
    pub trigger_executor: Arc<crate::trigger_engine::execution::TriggerExecutor>,
    pub retry: RetrySettings,
    pub registry: Registry,
    pub cwd: PathBuf,
    /// User home root (issue #66: `DaemonPaths::home`), resolved at the CLI
    /// boundary — file-command rescans take it instead of reading `$HOME`.
    pub home: PathBuf,
    /// Theway base dir (issue #66: `DaemonPaths::base`), resolved at the CLI
    /// boundary — kept alongside `home` so host paths stay explicit.
    pub base: PathBuf,
    /// Full daemon path context (issue #68): startup-fixed home/base/work_dir
    /// plus the dynamically replaceable extra skill dirs (`SetSkillDirs`).
    /// `cwd` / `home` / `base` above stay for the existing call sites.
    pub paths: DaemonPaths,
    pub session_id: String,
    pub log_path: Option<PathBuf>,
    pub tool_count: usize,
    pub feed_rx: mpsc::UnboundedReceiver<FeedUpdate>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    pub feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    pub main_run_rx: mpsc::UnboundedReceiver<String>,
    pub control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<UiControlPlanePrompt>>,
    pub dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    pub subagent_registry: theway_core::multiagent::registry::AgentJobRegistry,
    pub session_factory: SessionFactory,
    pub session_repo: Arc<SqliteSessionRepo>,
    pub current_session_state: Arc<Mutex<CurrentSessionState>>,
    pub panel_status: PanelStatus,
    /// `[orchestrator] thinking_summary` settings; `None` → thinking stays raw.
    pub thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    /// In-memory startup settings (issue #73): defaults merged with the
    /// controller's initial settings payload — the values startup previously
    /// read from `config.toml` (TUI scrollback cap, trigger poll interval,
    /// enabled builtin skills, …). Seeds the shared `GetConfig` view;
    /// runtime `Configure` updates merge into the view, not back into this
    /// startup snapshot.
    pub startup: crate::startup_config::StartupConfig,
}

/// Headless transport host for `thewayd` (gRPC / HTTP / MCP).
pub struct TurnHost {
    kernel: ReplKernel,
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
    daemon_config: Arc<std::sync::RwLock<WireDaemonConfig>>,
    /// Controller tool endpoint forwarder (issue #76): routes `ToolOps`
    /// calls to the TUI/controller's `ToolService` server.
    tool_ops: Arc<dyn ToolOps>,
    session_id: String,
    log_path: Option<PathBuf>,
    tool_count: usize,
    /// Process-level reload state (issue #50): registry / cwd / trigger
    /// executor for the `reload` tool and the revision counter published in
    /// sidebar snapshots.
    reload_runtime: Arc<ReloadRuntime>,

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
    feed_rx: Option<mpsc::UnboundedReceiver<FeedUpdate>>,
    /// Loopback sender for feed updates produced inside the host (thinking
    /// summarizer backfill); pairs with `feed_rx`.
    feed_tx: mpsc::UnboundedSender<FeedUpdate>,
    thinking_summary: Option<super::thinking_summary::ThinkingSummarySettings>,
    thinking_burst: super::thinking_summary::ThinkingBurst,
    main_run_rx: Option<mpsc::UnboundedReceiver<String>>,
    control_plane_prompt_rx: Option<mpsc::UnboundedReceiver<UiControlPlanePrompt>>,
    control_plane_prompt: Option<UiControlPlanePrompt>,
    model_catalog: Vec<ProviderGroup>,
    panel_status: PanelStatus,
    /// `[tui] max_feed_lines` (issue #73: from the in-memory StartupConfig /
    /// settings RPC; `None` → TUI built-in default).
    tui_max_feed_lines: Option<u64>,

    dag_engine: Arc<theway_core::multiagent::graph::engine::DagEngine>,
    subagent_registry: theway_core::multiagent::registry::AgentJobRegistry,
    session_factory: SessionFactory,
    session_repo: Arc<SqliteSessionRepo>,
    current_session_state: Arc<Mutex<CurrentSessionState>>,

    busy: bool,
    queued_turns: VecDeque<QueuedTurn>,
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
/// catalog. `0` when unknown — the TUI then hides the percentage indicator.
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
