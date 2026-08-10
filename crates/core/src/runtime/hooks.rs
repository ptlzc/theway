//! User-configured CLI hooks.
//!
//! Hooks are intentionally a coding-agent concern, not an agent-core behavior modifier:
//! they observe `AgentEvent`s and run best-effort side effects (shell commands and/or HTTP
//! webhooks). They never mutate agent state and failures are surfaced as diagnostics/logs,
//! not prompt failures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use theway_core::{
    AgentEvent, AgentListener, AgentMessage, HarnessEvent, HarnessListener, ThinkingLevel,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_SUMMARY_CHARS: usize = 2_000;

/// The theway base dir: `${THEWAY_DIR}` or `~/.theway`. Inlined (not via the server's
/// `config` module, which lives one layer up) so hooks stay engine-self-contained.
fn base_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("THEWAY_DIR") {
        return std::path::PathBuf::from(p);
    }
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".theway"))
        .unwrap_or_else(|| std::path::PathBuf::from(".theway"))
}

#[derive(Debug)]
pub struct LoadedHooks {
    pub runner: Arc<HookRunner>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug)]
pub struct HookRunner {
    rules: Vec<HookRule>,
    session_id: String,
    cwd: PathBuf,
    model_provider: String,
    model_id: String,
    thinking_level: String,
    client: reqwest::Client,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookEvent {
    AgentStart,
    AgentEnd,
    TurnStart,
    TurnEnd,
    MessageStart,
    MessageUpdate,
    MessageEnd,
    ToolStart,
    ToolUpdate,
    ToolEnd,
    Compaction,
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
    event: String,
    session_id: String,
    cwd: String,
    model_provider: String,
    model_id: String,
    thinking_level: String,
    source: Option<String>,
    message_kind: Option<String>,
    message_summary: Option<String>,
    assistant_event: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_is_error: Option<bool>,
    tool_args: Option<serde_json::Value>,
    tool_result_summary: Option<String>,
    compaction_trigger: Option<String>,
    compaction_tokens_before: Option<u64>,
    compaction_summary: Option<String>,
}

struct EventData {
    event: HookEvent,
    message_kind: Option<String>,
    message_summary: Option<String>,
    assistant_event: Option<String>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_is_error: Option<bool>,
    tool_args: Option<serde_json::Value>,
    tool_result_summary: Option<String>,
    compaction_trigger: Option<String>,
    compaction_tokens_before: Option<u64>,
    compaction_summary: Option<String>,
}

pub async fn load(
    cwd: &Path,
    session_id: impl Into<String>,
    model: Option<&theway_llm_provider::Model>,
    thinking_level: Option<ThinkingLevel>,
) -> LoadedHooks {
    let session_id = session_id.into();
    let (model_provider, model_id) = model
        .map(|m| (m.provider.0.clone(), m.id.clone()))
        .unwrap_or_else(|| ("".into(), "".into()));
    let thinking_level = thinking_level
        .map(|t| t.as_str().to_string())
        .unwrap_or_else(|| "off".into());

    let user_path = base_dir().join("hooks.toml");
    let project_path = cwd.join(".theway").join("hooks.toml");
    let mut diagnostics = Vec::new();
    let mut rules = Vec::new();

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

    let client = reqwest::Client::builder()
        .user_agent(format!("theway/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    LoadedHooks {
        runner: Arc::new(HookRunner {
            rules,
            session_id,
            cwd: cwd.to_path_buf(),
            model_provider,
            model_id,
            thinking_level,
            client,
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
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn listener(self: &Arc<Self>) -> AgentListener {
        let me = self.clone();
        Arc::new(move |event, cancel| {
            let me = me.clone();
            Box::pin(async move {
                me.handle_event(&event, cancel).await;
            })
        })
    }

    pub fn harness_listener(self: &Arc<Self>) -> HarnessListener {
        let me = self.clone();
        Arc::new(move |event| {
            let me = me.clone();
            tokio::spawn(async move {
                me.handle_harness_event(&event, CancellationToken::new())
                    .await;
            });
        })
    }

    pub async fn handle_event(&self, event: &AgentEvent, cancel: CancellationToken) {
        let Some(data) = EventData::from_agent_event(event) else {
            return;
        };
        self.handle_data(data, cancel).await;
    }

    pub async fn handle_harness_event(&self, event: &HarnessEvent, cancel: CancellationToken) {
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
            cwd: self.cwd.display().to_string(),
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
        let payload_path = write_payload_file(&payload_json).await?;

        let result = async {
            if let Some(command) = &rule.command {
                self.run_command(rule, command, payload, &payload_path, cancel.clone())
                    .await?;
            }
            if let Some(url) = &rule.webhook {
                self.run_webhook(rule, url, &payload_json, cancel.clone())
                    .await?;
            }
            anyhow::Ok(())
        }
        .await;

        let _ = tokio::fs::remove_file(&payload_path).await;
        result
    }

    async fn run_command(
        &self,
        rule: &HookRule,
        command: &str,
        payload: &HookPayload,
        payload_path: &Path,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        // Previously: `cmd.output()` raced against `tokio::time::timeout` + cancel via
        // `select!`. Either non-completion branch left `sh -c` running in the background
        // (along with anything it spawned), so a hook that ran `(slow_thing) & wait` would
        // leak descendants past its declared `timeout_ms`. Mirrors the bash-tool fix in
        // PR #41 and the `NativeEnv::exec` fix in PR #40: spawn explicitly, put the child
        // in its own process group on Unix via `setsid`, and `killpg(pgid, SIGKILL)` the
        // whole tree on timeout / cancel. `kill_on_drop(true)` is the cross-platform
        // backstop.
        let timeout = Duration::from_millis(rule.timeout_ms);
        let mut cmd = Command::new(shell_program());
        cmd.arg(shell_arg())
            .arg(command)
            .current_dir(self.cwd_for(rule))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env_for(payload, payload_path))
            .kill_on_drop(true);

        #[cfg(unix)]
        {
            // SAFETY: `setsid` is async-signal-safe per POSIX and has no Rust state to
            // invalidate. The child becomes session and process-group leader; SIGKILL to
            // `-pgid` then targets the whole tree we just spawned. `tokio::process::Command`
            // exposes `pre_exec` as an inherent method so no trait import is needed.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        let child = cmd.spawn().map_err(|e| anyhow::anyhow!("spawn: {e}"))?;
        let child_pid = child.id();

        // Race the wait against the rule's timeout and the cancel token. `biased` puts
        // cancel first so a user Ctrl-C wins same-tick ties over the timeout.
        let outcome: HookOutcome = {
            let wait = child.wait_with_output();
            tokio::pin!(wait);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => HookOutcome::Cancelled,
                res = tokio::time::timeout(timeout, &mut wait) => match res {
                    Ok(out) => HookOutcome::Completed(out),
                    Err(_) => HookOutcome::TimedOut,
                },
            }
        };

        match outcome {
            HookOutcome::Completed(Ok(output)) => {
                if !output.status.success() {
                    anyhow::bail!(
                        "command exited {}: {}",
                        output.status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                if !output.stdout.is_empty() {
                    tracing::debug!(
                        "hook command stdout: {}",
                        String::from_utf8_lossy(&output.stdout).trim()
                    );
                }
                if !output.stderr.is_empty() {
                    tracing::debug!(
                        "hook command stderr: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Ok(())
            }
            HookOutcome::Completed(Err(e)) => Err(anyhow::anyhow!(e)),
            HookOutcome::TimedOut => {
                terminate_hook_tree(child_pid).await;
                anyhow::bail!("timed out after {}ms", rule.timeout_ms);
            }
            HookOutcome::Cancelled => {
                terminate_hook_tree(child_pid).await;
                anyhow::bail!("cancelled");
            }
        }
    }
}

/// Outcome of one `run_command` race. Lifted out of `tokio::select!` so the match below
/// can spell the kill-tree path explicitly per branch rather than mixing it into the
/// select arms.
enum HookOutcome {
    Completed(std::io::Result<std::process::Output>),
    TimedOut,
    Cancelled,
}

/// Best-effort SIGKILL of the hook child's whole process group on Unix. On non-Unix targets
/// this is a no-op (the `kill_on_drop(true)` set on the `Command` is the only kill path
/// when the wait future is dropped). The pid was snapshotted with `child.id()` before the
/// wait future consumed the handle.
async fn terminate_hook_tree(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        // SAFETY: SIGKILL on a pid we just observed via `child.id()`. `killpg` returning
        // `ESRCH` (group already gone) is benign and we don't act on the return.
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    let _ = pid;
}

impl HookRunner {
    async fn run_webhook(
        &self,
        rule: &HookRule,
        url: &str,
        payload_json: &str,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut req = self
            .client
            .post(url)
            .timeout(Duration::from_millis(rule.timeout_ms))
            .header("Content-Type", "application/json")
            .body(payload_json.to_string());
        for (k, v) in &rule.headers {
            req = req.header(k, v);
        }
        let resp = tokio::select! {
            r = req.send() => r?,
            _ = cancel.cancelled() => {
                anyhow::bail!("cancelled");
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "webhook status {status}: {}",
                text.chars().take(500).collect::<String>()
            );
        }
        Ok(())
    }

    fn cwd_for(&self, rule: &HookRule) -> PathBuf {
        match rule.cwd {
            HookCwd::Project => self.cwd.clone(),
            HookCwd::ThewayHarness => base_dir(),
            HookCwd::Home => directories::BaseDirs::new()
                .map(|d| d.home_dir().to_path_buf())
                .unwrap_or_else(|| self.cwd.clone()),
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

impl HookEvent {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "agent_start" => Some(Self::AgentStart),
            "agent_end" => Some(Self::AgentEnd),
            "turn_start" => Some(Self::TurnStart),
            "turn_end" => Some(Self::TurnEnd),
            "message_start" => Some(Self::MessageStart),
            "message_update" => Some(Self::MessageUpdate),
            "message_end" => Some(Self::MessageEnd),
            "tool_start" => Some(Self::ToolStart),
            "tool_update" => Some(Self::ToolUpdate),
            "tool_end" => Some(Self::ToolEnd),
            "compaction" => Some(Self::Compaction),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::AgentStart => "agent_start",
            Self::AgentEnd => "agent_end",
            Self::TurnStart => "turn_start",
            Self::TurnEnd => "turn_end",
            Self::MessageStart => "message_start",
            Self::MessageUpdate => "message_update",
            Self::MessageEnd => "message_end",
            Self::ToolStart => "tool_start",
            Self::ToolUpdate => "tool_update",
            Self::ToolEnd => "tool_end",
            Self::Compaction => "compaction",
        }
    }
}

impl EventData {
    fn from_agent_event(event: &AgentEvent) -> Option<Self> {
        match event {
            AgentEvent::AgentStart => Some(Self::basic(HookEvent::AgentStart)),
            AgentEvent::AgentEnd { .. } => Some(Self::basic(HookEvent::AgentEnd)),
            AgentEvent::TurnStart => Some(Self::basic(HookEvent::TurnStart)),
            AgentEvent::TurnEnd { message, .. } => {
                let mut d = Self::basic(HookEvent::TurnEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::MessageStart { message } => {
                let mut d = Self::basic(HookEvent::MessageStart);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::MessageUpdate {
                message,
                assistant_message_event,
            } => {
                let mut d = Self::basic(HookEvent::MessageUpdate);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                d.assistant_event = Some(assistant_event_name(assistant_message_event).into());
                Some(d)
            }
            AgentEvent::MessageEnd { message } => {
                let mut d = Self::basic(HookEvent::MessageEnd);
                d.message_kind = Some(message_kind(message));
                d.message_summary = Some(message_summary(message));
                Some(d)
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                let mut d = Self::basic(HookEvent::ToolStart);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_args = Some(args.clone());
                Some(d)
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial_result,
            } => {
                let mut d = Self::basic(HookEvent::ToolUpdate);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_args = Some(args.clone());
                d.tool_result_summary = Some(result_summary(partial_result));
                Some(d)
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let mut d = Self::basic(HookEvent::ToolEnd);
                d.tool_call_id = Some(tool_call_id.clone());
                d.tool_name = Some(tool_name.clone());
                d.tool_is_error = Some(*is_error);
                d.tool_result_summary = Some(result_summary(result));
                Some(d)
            }
            // Issue #110: control-plane prompt observability event. Embedder-side
            // hook-script bridge does not currently surface this; defer to a follow-up
            // PR once we have a concrete bridge consumer that wants it.
            AgentEvent::ControlPlanePromptResolved { .. } => None,
        }
    }

    fn basic(event: HookEvent) -> Self {
        Self {
            event,
            message_kind: None,
            message_summary: None,
            assistant_event: None,
            tool_call_id: None,
            tool_name: None,
            tool_is_error: None,
            tool_args: None,
            tool_result_summary: None,
            compaction_trigger: None,
            compaction_tokens_before: None,
            compaction_summary: None,
        }
    }

    fn from_harness_event(event: &HarnessEvent) -> Option<Self> {
        match event {
            HarnessEvent::Compaction {
                from_hook,
                summary,
                tokens_before,
            } => {
                let mut d = Self::basic(HookEvent::Compaction);
                d.compaction_trigger = Some(compaction_trigger(*from_hook).into());
                d.compaction_tokens_before = Some(*tokens_before);
                d.compaction_summary = Some(truncate(summary));
                Some(d)
            }
            HarnessEvent::SessionStart { .. }
            | HarnessEvent::Branch { .. }
            | HarnessEvent::TriggerHandlingStart { .. }
            | HarnessEvent::TriggerHandled { .. }
            | HarnessEvent::TriggerPromptRequest { .. }
            | HarnessEvent::PersistenceError { .. }
            | HarnessEvent::TriggerExecutionStarted { .. }
            | HarnessEvent::TriggerCompleted { .. }
            | HarnessEvent::TriggerFailed { .. }
            | HarnessEvent::TriggerPromoted { .. }
            | HarnessEvent::TriggerRequestsMainRun { .. }
            | HarnessEvent::PromotionPending { .. }
            | HarnessEvent::TurnEnded { .. }
            | HarnessEvent::SkillsReloaded { .. } => None,
        }
    }
}

fn compaction_trigger(from_hook: bool) -> &'static str {
    // In current AgentHarness call sites, true is the explicit `force_compact` path used by
    // `/compact`; false is the threshold-based automatic compaction path.
    if from_hook { "manual" } else { "auto" }
}

fn env_for(payload: &HookPayload, payload_path: &Path) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("THEWAY_HOOK_EVENT".into(), payload.event.clone());
    env.insert(
        "THEWAY_HOOK_PAYLOAD".into(),
        payload_path.display().to_string(),
    );
    env.insert("THEWAY_SESSION_ID".into(), payload.session_id.clone());
    env.insert("THEWAY_CWD".into(), payload.cwd.clone());
    env.insert(
        "THEWAY_MODEL_PROVIDER".into(),
        payload.model_provider.clone(),
    );
    env.insert("THEWAY_MODEL_ID".into(), payload.model_id.clone());
    env.insert(
        "THEWAY_THINKING_LEVEL".into(),
        payload.thinking_level.clone(),
    );
    if let Some(v) = &payload.message_kind {
        env.insert("THEWAY_MESSAGE_KIND".into(), v.clone());
    }
    if let Some(v) = &payload.assistant_event {
        env.insert("THEWAY_ASSISTANT_EVENT".into(), v.clone());
    }
    if let Some(v) = &payload.tool_call_id {
        env.insert("THEWAY_TOOL_CALL_ID".into(), v.clone());
    }
    if let Some(v) = &payload.tool_name {
        env.insert("THEWAY_TOOL_NAME".into(), v.clone());
    }
    if let Some(v) = payload.tool_is_error {
        env.insert("THEWAY_TOOL_IS_ERROR".into(), v.to_string());
    }
    if let Some(v) = &payload.compaction_trigger {
        env.insert("THEWAY_COMPACTION_TRIGGER".into(), v.clone());
    }
    if let Some(v) = payload.compaction_tokens_before {
        env.insert("THEWAY_COMPACTION_TOKENS_BEFORE".into(), v.to_string());
    }
    env
}

async fn write_payload_file(payload_json: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join("theway-hooks");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, payload_json).await?;
    Ok(path)
}

fn message_kind(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Llm(theway_llm_provider::Message::User(_)) => "user".into(),
        AgentMessage::Llm(theway_llm_provider::Message::Assistant(_)) => "assistant".into(),
        AgentMessage::Llm(theway_llm_provider::Message::ToolResult(_)) => "tool_result".into(),
        AgentMessage::Custom(c) => c.role.clone(),
    }
}

fn message_summary(message: &AgentMessage) -> String {
    let text = match message {
        AgentMessage::Llm(theway_llm_provider::Message::User(u)) => match &u.content {
            theway_llm_provider::UserContent::Text(t) => t.clone(),
            theway_llm_provider::UserContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
                    theway_llm_provider::UserContentBlock::Image(i) => {
                        format!("<image {}>", i.mime_type)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        AgentMessage::Llm(theway_llm_provider::Message::Assistant(a)) => a
            .content
            .iter()
            .map(|b| match b {
                theway_llm_provider::ContentBlock::Text(t) => t.text.clone(),
                theway_llm_provider::ContentBlock::Thinking(_) => "<thinking>".into(),
                theway_llm_provider::ContentBlock::ToolCall(tc) => {
                    format!("<tool_call {}>", tc.name)
                }
                theway_llm_provider::ContentBlock::Image(i) => format!("<image {}>", i.mime_type),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        AgentMessage::Llm(theway_llm_provider::Message::ToolResult(t)) => t
            .content
            .iter()
            .map(|b| match b {
                theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
                theway_llm_provider::UserContentBlock::Image(i) => {
                    format!("<image {}>", i.mime_type)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        AgentMessage::Custom(c) => serde_json::to_string(&c.payload).unwrap_or_default(),
    };
    truncate(&text)
}

fn result_summary(result: &theway_core::AgentToolResult) -> String {
    let text = result
        .content
        .iter()
        .map(|b| match b {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            theway_llm_provider::UserContentBlock::Image(i) => format!("<image {}>", i.mime_type),
        })
        .collect::<Vec<_>>()
        .join("\n");
    truncate(&text)
}

fn assistant_event_name(ev: &theway_llm_provider::AssistantMessageEvent) -> &'static str {
    match ev {
        theway_llm_provider::AssistantMessageEvent::Start { .. } => "start",
        theway_llm_provider::AssistantMessageEvent::TextStart { .. } => "text_start",
        theway_llm_provider::AssistantMessageEvent::TextDelta { .. } => "text_delta",
        theway_llm_provider::AssistantMessageEvent::TextEnd { .. } => "text_end",
        theway_llm_provider::AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
        theway_llm_provider::AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
        theway_llm_provider::AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
        theway_llm_provider::AssistantMessageEvent::ToolCallStart { .. } => "tool_call_start",
        theway_llm_provider::AssistantMessageEvent::ToolCallDelta { .. } => "tool_call_delta",
        theway_llm_provider::AssistantMessageEvent::ToolCallEnd { .. } => "tool_call_end",
        theway_llm_provider::AssistantMessageEvent::Done { .. } => "done",
        theway_llm_provider::AssistantMessageEvent::Error { .. } => "error",
    }
}

fn truncate(s: &str) -> String {
    if s.chars().count() <= MAX_SUMMARY_CHARS {
        return s.to_string();
    }
    let mut out = s.chars().take(MAX_SUMMARY_CHARS).collect::<String>();
    out.push('…');
    out
}

#[cfg(unix)]
fn shell_program() -> &'static str {
    "sh"
}

#[cfg(unix)]
fn shell_arg() -> &'static str {
    "-c"
}

#[cfg(windows)]
fn shell_program() -> &'static str {
    "cmd"
}

#[cfg(windows)]
fn shell_arg() -> &'static str {
    "/C"
}

#[cfg(test)]
// Test files live in `tests/runtime/hooks/` (mirror of `src/runtime/`), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
#[path = "../../tests/runtime/hooks/mod.rs"]
mod tests;
