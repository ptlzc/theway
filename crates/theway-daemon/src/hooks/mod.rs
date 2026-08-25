//! User-configured CLI hooks.
//!
//! Hooks are intentionally a coding-agent concern, not an agent-core behavior modifier:
//! they observe `LoopEvent`s and run best-effort side effects (shell commands and/or HTTP
//! webhooks). They never mutate agent state and failures are surfaced as diagnostics/logs,
//! not prompt failures.
//!
//! Layout: [`event`] holds the hook event taxonomy and agent/harness-event → payload-data
//! mapping; [`utils`] holds the execution and summary helpers shared by that mapping and
//! the runner. This module keeps rule loading (`load`/`read_file`/`push_rules`) and the
//! [`HookRunner`] execution logic.
//!
//! # Side-effect seams
//!
//! The daemon owns the hook runtime. Command execution and webhook delivery
//! are injected at construction as [`HookCommandExecutor`] /
//! [`HookWebhookSender`] closures ([`HookExecutors`]); the implementations
//! live in [`crate::hook_executors`], where the command executor reuses the
//! single process-group-kill primitive shared by the bash tool, the
//! exec_shell family and native env. Without injected executors, side-effect
//! rules are skipped and reported in the load diagnostics.

pub mod event;
pub mod utils;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
#[cfg(test)] // Only used by the bridged unit tests (`tests/hooks`) via `use super::*`.
use theway_core::AgentMessage;
use theway_core::{LoopEvent, LoopListener, SessionEvent, SessionListener, ThinkingLevel};
use tokio_util::sync::CancellationToken;

use event::{EventData, HookEvent};
use utils::{env_for, write_payload_file};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug)]
pub struct LoadedHooks {
    pub runner: Arc<HookRunner>,
    pub diagnostics: Vec<String>,
}

/// Command-execution seam for hook rules. The daemon owns the implementation
/// (spawn `sh -c` with the given env, whole-process-tree kill on timeout/cancel)
/// in [`crate::hook_executors`], routing it through the single `setsid`/`killpg`
/// primitive shared by the bash tool, the exec_shell family and native env.
/// Returns captured stdout/stderr on success.
pub type HookCommandExecutor = Arc<
    dyn Fn(
            String,                   // command
            PathBuf,                  // resolved cwd
            BTreeMap<String, String>, // env for the child
            Duration,                 // timeout
            CancellationToken,        // cancel
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<HookCommandOutput>> + Send>>
        + Send
        + Sync,
>;

/// Captured output of a successful hook command run, returned to the runner for
/// debug logging.
#[derive(Debug, Default)]
pub struct HookCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Webhook-send seam for hook rules. The runner assembles the payload and headers;
/// the HTTP delivery implementation (POST + timeout + cancel race + status check)
/// lives in [`crate::hook_executors`].
pub type HookWebhookSender = Arc<
    dyn Fn(
            String,                   // url
            String,                   // payload JSON body
            BTreeMap<String, String>, // extra headers
            Duration,                 // timeout
            CancellationToken,        // cancel
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Side-effect executors injected when constructing [`HookRunner`] (via [`load`]).
/// `None` = not injected: rules of that kind are skipped at runtime and a diagnostic
/// is recorded at load time.
#[derive(Default, Clone)]
pub struct HookExecutors {
    pub command: Option<HookCommandExecutor>,
    pub webhook: Option<HookWebhookSender>,
}

impl std::fmt::Debug for HookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRunner")
            .field("rules", &self.rules)
            .field("session_id", &self.session_id)
            .field("work_dir", &self.work_dir)
            .field("base", &self.base)
            .field("home", &self.home)
            .field("model_provider", &self.model_provider)
            .field("model_id", &self.model_id)
            .field("thinking_level", &self.thinking_level)
            .field(
                "command_executor",
                &self.command_executor.as_ref().map(|_| "<closure>"),
            )
            .field(
                "webhook_sender",
                &self.webhook_sender.as_ref().map(|_| "<closure>"),
            )
            .finish()
    }
}

pub struct HookRunner {
    rules: Vec<HookRule>,
    session_id: String,
    work_dir: PathBuf,
    base: PathBuf,
    home: PathBuf,
    model_provider: String,
    model_id: String,
    thinking_level: String,
    command_executor: Option<HookCommandExecutor>,
    webhook_sender: Option<HookWebhookSender>,
}

#[derive(Clone, Debug)]
struct HookRule {
    event: HookEvent,
    command: Option<String>,
    webhook: Option<String>,
    headers: BTreeMap<String, String>,
    timeout_ms: u64,
    cwd: HookCwd,
    on_failure: OnFailure,
    tool: Option<String>,
    source: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HookCwd {
    Project,
    #[serde(rename = "theway")]
    ThewayHarness,
    Home,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OnFailure {
    Warn,
    Ignore,
}

#[derive(Debug, Deserialize)]
struct HooksFile {
    #[serde(default)]
    allow_project_hooks: bool,
    #[serde(default, rename = "hook")]
    hooks: Vec<HookRuleConfig>,
}

#[derive(Debug, Deserialize)]
struct HookRuleConfig {
    event: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    webhook: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    cwd: Option<HookCwd>,
    #[serde(default)]
    on_failure: Option<OnFailure>,
    #[serde(default)]
    tool: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HookPayload {
    pub(super) event: String,
    pub(super) session_id: String,
    pub(super) cwd: String,
    pub(super) model_provider: String,
    pub(super) model_id: String,
    pub(super) thinking_level: String,
    pub(super) source: Option<String>,
    pub(super) message_kind: Option<String>,
    pub(super) message_summary: Option<String>,
    pub(super) assistant_event: Option<String>,
    pub(super) tool_call_id: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) tool_is_error: Option<bool>,
    pub(super) tool_args: Option<serde_json::Value>,
    pub(super) tool_result_summary: Option<String>,
    pub(super) compaction_trigger: Option<String>,
    pub(super) compaction_tokens_before: Option<u64>,
    pub(super) compaction_summary: Option<String>,
}

pub async fn load(
    paths: &crate::DaemonPaths,
    session_id: impl Into<String>,
    model: Option<&theway_llm_provider::Model>,
    thinking_level: Option<ThinkingLevel>,
    executors: HookExecutors,
) -> LoadedHooks {
    load_with(paths, session_id, model, thinking_level, executors, true).await
}

/// Same as [`load`] with an explicit local-file seam (issue #73):
/// `read_local_files = false` skips the `hooks.toml` scans entirely and
/// returns a rule-less runner — the shape startup needs once a controller
/// provisions hooks through the settings RPC instead of local files.
/// TODO(#73): wire the controller-provisioned rules through here.
pub async fn load_with(
    paths: &crate::DaemonPaths,
    session_id: impl Into<String>,
    model: Option<&theway_llm_provider::Model>,
    thinking_level: Option<ThinkingLevel>,
    executors: HookExecutors,
    read_local_files: bool,
) -> LoadedHooks {
    let session_id = session_id.into();
    let (model_provider, model_id) = model
        .map(|m| (m.provider.0.clone(), m.id.clone()))
        .unwrap_or_else(|| ("".into(), "".into()));
    let thinking_level = thinking_level
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "off".into());

    let user_path = paths.base.join("hooks.toml");
    let project_path = paths.work_dir.join(".theway").join("hooks.toml");
    let mut diagnostics = Vec::new();
    let mut rules = Vec::new();

    if read_local_files {
        let user_file = read_file(&user_path, "user", &mut diagnostics).await;
        let allow_project = std::env::var("THEWAY_ALLOW_PROJECT_HOOKS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
            || user_file
                .as_ref()
                .map(|f| f.allow_project_hooks)
                .unwrap_or(false);

        if let Some(file) = user_file {
            push_rules(file, "user", &mut rules, &mut diagnostics);
        }

        if project_path.exists() {
            if allow_project {
                if let Some(file) = read_file(&project_path, "project", &mut diagnostics).await {
                    push_rules(file, "project", &mut rules, &mut diagnostics);
                }
            } else {
                diagnostics.push(format!(
                    "project hooks ignored at {}; set allow_project_hooks = true in {} or THEWAY_ALLOW_PROJECT_HOOKS=1",
                    project_path.display(),
                    user_path.display()
                ));
            }
        }
    }

    if executors.command.is_none() && rules.iter().any(|r| r.command.is_some()) {
        diagnostics.push(
            "hooks: command rules loaded but no command executor was injected — command side \
             effects will be skipped (no side-effect executors injected)"
                .into(),
        );
    }
    if executors.webhook.is_none() && rules.iter().any(|r| r.webhook.is_some()) {
        diagnostics.push(
            "hooks: webhook rules loaded but no webhook sender was injected — webhook side \
             effects will be skipped (no side-effect executors injected)"
                .into(),
        );
    }

    LoadedHooks {
        runner: Arc::new(HookRunner {
            rules,
            session_id,
            work_dir: paths.work_dir.to_path_buf(),
            base: paths.base.to_path_buf(),
            home: paths.home.to_path_buf(),
            model_provider,
            model_id,
            thinking_level,
            command_executor: executors.command,
            webhook_sender: executors.webhook,
        }),
        diagnostics,
    }
}

async fn read_file(path: &Path, label: &str, diagnostics: &mut Vec<String>) -> Option<HooksFile> {
    if !path.exists() {
        return None;
    }
    let text = match tokio::fs::read_to_string(path).await {
        Ok(text) => text,
        Err(e) => {
            diagnostics.push(format!(
                "hooks {label}: read {} failed: {e}",
                path.display()
            ));
            return None;
        }
    };
    match toml::from_str::<HooksFile>(&text) {
        Ok(file) => Some(file),
        Err(e) => {
            diagnostics.push(format!(
                "hooks {label}: parse {} failed: {e}",
                path.display()
            ));
            None
        }
    }
}

fn push_rules(
    file: HooksFile,
    source: &str,
    rules: &mut Vec<HookRule>,
    diagnostics: &mut Vec<String>,
) {
    for (idx, cfg) in file.hooks.into_iter().enumerate() {
        if cfg.enabled == Some(false) {
            continue;
        }
        let event = match HookEvent::parse(&cfg.event) {
            Some(event) => event,
            None => {
                diagnostics.push(format!(
                    "hooks {source}: hook #{} has unknown event {:?}",
                    idx + 1,
                    cfg.event
                ));
                continue;
            }
        };
        if cfg.command.as_deref().unwrap_or("").trim().is_empty() && cfg.webhook.is_none() {
            diagnostics.push(format!(
                "hooks {source}: hook #{} has neither command nor webhook",
                idx + 1
            ));
            continue;
        }
        rules.push(HookRule {
            event,
            command: cfg.command.filter(|s| !s.trim().is_empty()),
            webhook: cfg.webhook,
            headers: cfg.headers,
            timeout_ms: cfg.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
            cwd: cfg.cwd.unwrap_or(HookCwd::Project),
            on_failure: cfg.on_failure.unwrap_or(OnFailure::Warn),
            tool: cfg.tool,
            source: source.to_string(),
        });
    }
}

impl HookRunner {
    /// Clone the loaded rules, explicit paths, and executors into a session-bound runner.
    /// Missing model/thinking values use the same blank/off defaults as [`load_with`].
    pub fn for_session(
        &self,
        session_id: impl Into<String>,
        model: Option<&theway_llm_provider::Model>,
        thinking_level: Option<ThinkingLevel>,
    ) -> Self {
        let (model_provider, model_id) = model
            .map(|m| (m.provider.0.clone(), m.id.clone()))
            .unwrap_or_else(|| ("".into(), "".into()));
        let thinking_level = thinking_level
            .map(|t| t.as_str().to_string())
            .unwrap_or_else(|| "off".into());
        Self {
            rules: self.rules.clone(),
            session_id: session_id.into(),
            work_dir: self.work_dir.clone(),
            base: self.base.clone(),
            home: self.home.clone(),
            model_provider,
            model_id,
            thinking_level,
            command_executor: self.command_executor.clone(),
            webhook_sender: self.webhook_sender.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn listener(self: &Arc<Self>) -> LoopListener {
        let me = self.clone();
        Arc::new(move |event, cancel| {
            let me = me.clone();
            Box::pin(async move {
                me.handle_event(&event, cancel).await;
            })
        })
    }

    pub fn harness_listener(self: &Arc<Self>) -> SessionListener {
        let me = self.clone();
        Arc::new(move |event| {
            let me = me.clone();
            tokio::spawn(async move {
                me.handle_harness_event(&event, CancellationToken::new())
                    .await;
            });
        })
    }

    pub async fn handle_event(&self, event: &LoopEvent, cancel: CancellationToken) {
        let Some(data) = EventData::from_agent_event(event) else {
            return;
        };
        self.handle_data(data, cancel).await;
    }

    pub async fn handle_harness_event(&self, event: &SessionEvent, cancel: CancellationToken) {
        let Some(data) = EventData::from_harness_event(event) else {
            return;
        };
        self.handle_data(data, cancel).await;
    }

    async fn handle_data(&self, data: EventData, cancel: CancellationToken) {
        let matching = self
            .rules
            .iter()
            .filter(|rule| rule.matches(&data))
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return;
        }

        for rule in matching {
            if cancel.is_cancelled() {
                return;
            }
            let payload = self.payload_for(rule, &data);
            if let Err(e) = self.run_rule(rule, &payload, cancel.clone()).await
                && matches!(rule.on_failure, OnFailure::Warn)
            {
                tracing::warn!("hook {} {} failed: {e}", rule.source, rule.event.as_str());
            }
        }
    }

    fn payload_for(&self, rule: &HookRule, data: &EventData) -> HookPayload {
        HookPayload {
            event: data.event.as_str().to_string(),
            session_id: self.session_id.clone(),
            cwd: self.work_dir.display().to_string(),
            model_provider: self.model_provider.clone(),
            model_id: self.model_id.clone(),
            thinking_level: self.thinking_level.clone(),
            source: Some(rule.source.clone()),
            message_kind: data.message_kind.clone(),
            message_summary: data.message_summary.clone(),
            assistant_event: data.assistant_event.clone(),
            tool_call_id: data.tool_call_id.clone(),
            tool_name: data.tool_name.clone(),
            tool_is_error: data.tool_is_error,
            tool_args: data.tool_args.clone(),
            tool_result_summary: data.tool_result_summary.clone(),
            compaction_trigger: data.compaction_trigger.clone(),
            compaction_tokens_before: data.compaction_tokens_before,
            compaction_summary: data.compaction_summary.clone(),
        }
    }

    async fn run_rule(
        &self,
        rule: &HookRule,
        payload: &HookPayload,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let payload_json = serde_json::to_string(payload)?;

        if let Some(command) = &rule.command {
            if let Some(executor) = &self.command_executor {
                let payload_path = write_payload_file(&payload_json).await?;
                let run = executor(
                    command.clone(),
                    self.cwd_for(rule),
                    env_for(payload, &payload_path),
                    Duration::from_millis(rule.timeout_ms),
                    cancel.clone(),
                )
                .await;
                let _ = tokio::fs::remove_file(&payload_path).await;
                match run {
                    Ok(out) => {
                        if !out.stdout.is_empty() {
                            tracing::debug!("hook command stdout: {}", out.stdout.trim());
                        }
                        if !out.stderr.is_empty() {
                            tracing::debug!("hook command stderr: {}", out.stderr.trim());
                        }
                    }
                    Err(e) => return Err(e),
                }
            } else {
                tracing::debug!(
                    "hook {} {}: command side effect skipped (no command executor injected)",
                    rule.source,
                    rule.event.as_str()
                );
            }
        }

        if let Some(url) = &rule.webhook {
            if let Some(sender) = &self.webhook_sender {
                sender(
                    url.clone(),
                    payload_json.clone(),
                    rule.headers.clone(),
                    Duration::from_millis(rule.timeout_ms),
                    cancel.clone(),
                )
                .await?;
            } else {
                tracing::debug!(
                    "hook {} {}: webhook side effect skipped (no webhook sender injected)",
                    rule.source,
                    rule.event.as_str()
                );
            }
        }

        Ok(())
    }

    fn cwd_for(&self, rule: &HookRule) -> PathBuf {
        match rule.cwd {
            HookCwd::Project => self.work_dir.clone(),
            HookCwd::ThewayHarness => self.base.clone(),
            HookCwd::Home => self.home.clone(),
        }
    }
}

impl HookRule {
    fn matches(&self, data: &EventData) -> bool {
        if self.event != data.event {
            return false;
        }
        if let Some(tool) = &self.tool
            && data.tool_name.as_deref() != Some(tool.as_str())
        {
            return false;
        }
        true
    }
}

#[cfg(test)]
// Test files live in `tests/hooks/` (mirror of src), pulled in by path so
// they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("hooks");
