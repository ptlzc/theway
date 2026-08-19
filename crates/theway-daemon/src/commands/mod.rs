//! Slash-command registry — daemon layer.
//!
//! The command framework (Registry / SlashCommand / CommandOutcome / CommandCtx,
//! `parse`, pure helpers) lives in `theway_transport::commands` (the shared
//! client-contract zone); command implementations live with their owners — the client-local
//! set (quit/clear/help + interactive login) in `theway-tui`, the runtime set here. This
//! module keeps the daemon-side surface:
//!
//! - re-exports of the shared framework so existing `crate::commands::…` /
//!   `theway_daemon::commands::…` paths keep resolving (readline, tests);
//! - [`DaemonCtx`] — the daemon-only context extras carried by the framework's generic
//!   `CommandCtx<'_, DaemonCtx>` (the trigger executor handle);
//! - the daemon command implementations in submodules: [`skills`] / [`skill_cmd`] (skill
//!   management), [`model`] (`/model`, `/thinking`, `/cost` + model-catalog help), [`goal`]
//!   (`/goal`, `/goal-start`), [`session`] (session lifecycle), [`triggers`] (automation),
//!   and [`misc`] (everything else); all implement `SlashCommand<DaemonCtx>`;
//! - the [`Registry`] wrapper over the shared framework's generic registry with
//!   [`Registry::with_daemon_commands`]: registers the daemon runtime command set
//!   (the TUI keeps its own client-local set in `theway-tui`);
//! - [`dispatch`], which converts the daemon-shaped [`CommandCtx`] into the framework's
//!   generic context (extras = [`DaemonCtx`]) and routes `/help` + skill shortcuts.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::execution::{NotificationStatusSnapshot, RunningTriggerState};
use crate::trigger_engine::notification_hook::{HookState, NotificationHookStatus};
use async_trait::async_trait;
use serde_json::json;
use theway_core::{AgentHarness, AgentTool, SessionTreeEntry, Skill, SkillSource, ThinkingLevel};
use theway_daemon::runtime_storage::{RuntimeStorage, local_runtime_storage};
use theway_llm_provider::{Model, Provider, UserContentBlock, get_model};
use tokio_util::sync::CancellationToken;

// Framework lives in the transport crate's shared zone. Re-exported so existing
// `commands::…` call sites (ui, main, readline, model_picker, tests) keep their paths.
// The allow keeps the `commands::…` paths alive in the path-included e2e test
// crate, where this module is private and some re-exports have no in-tree user.
#[allow(unused_imports)]
pub use theway_transport::auth::{model_credential_hint, save_api_key};
// Compatibility surface for path-bridged tests. Production daemon commands use the
// instance-owned `CommandOutput` below.
#[cfg(test)]
pub use theway_transport::commands::console;
#[allow(unused_imports)]
pub use theway_transport::commands::{
    CommandOutcome, SlashCommand, WebRelayAction, attach_skill_prompt, cli_model_help_text, parse,
    parse_model_spec,
};

/// One daemon instance's slash-command output destination.
#[derive(Clone)]
pub struct CommandOutput {
    emit: Arc<dyn Fn(String) + Send + Sync>,
}

impl CommandOutput {
    pub fn new(emit: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self {
            emit: Arc::new(emit),
        }
    }

    pub fn stdout() -> Self {
        Self::new(|line| println!("{line}"))
    }

    fn emit_line(&self, line: String) {
        (self.emit)(line);
    }

    async fn scope<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        ACTIVE_COMMAND_OUTPUT.scope(self.clone(), future).await
    }
}

impl Default for CommandOutput {
    fn default() -> Self {
        #[cfg(test)]
        {
            Self::new(console::emit_line)
        }
        #[cfg(not(test))]
        Self::stdout()
    }
}

tokio::task_local! {
    static ACTIVE_COMMAND_OUTPUT: CommandOutput;
}

fn emit_command_line(line: String) {
    let fallback = line.clone();
    if ACTIVE_COMMAND_OUTPUT
        .try_with(|output| output.emit_line(line))
        .is_err()
    {
        CommandOutput::default().emit_line(fallback);
    }
}

/// Drop-in replacement for `println!` inside this module. The dispatcher scopes it to the
/// output destination owned by the current command registry.
macro_rules! cprintln {
    () => { $crate::commands::emit_command_line(String::new()) };
    ($($arg:tt)*) => { $crate::commands::emit_command_line(std::format!($($arg)*)) };
}

pub mod auth;
pub mod goal;
pub mod misc;
pub mod model;
pub mod session;
pub mod skill_cmd;
pub mod skills;
pub mod triggers;

// The module's public API stays rooted here: items that moved into submodules are
// re-exported so existing `commands::…` call sites (ui, main, readline, model_picker,
// tests) keep their paths.
pub use misc::print_help_with_skills;
// No in-tree callers, but keep the pre-split `commands::print_help` path alive.
#[allow(unused_imports)]
pub use misc::print_help;
// Only `tests/commands.rs` calls through these `commands::…` paths; the allow keeps the
// pre-split pub(crate) surface without tripping unused_imports in non-test builds.
#[allow(unused_imports)]
pub(crate) use triggers::{render_cron_jobs, render_dynamic_trigger_rules, render_triggers_status};

// Daemon command implementations registered by `Registry::with_daemon_commands` (the
// client-local set lives in the TUI crate's `local_commands`).
use goal::{GoalCommand, GoalStartCommand};
use misc::{
    BugReportCommand, CompactCommand, DiagCommand, FindCommand, HistoryCommand, TemplateCommand,
    WebConnectCommand, WebDisconnectCommand,
};
use model::{CostCommand, ModelCommand, ThinkingCommand};
use session::{ForkCommand, NameCommand, SaveCommand, SessionCommand, ShareCommand, UndoCommand};
use skill_cmd::SkillCommand;
use skills::SkillsCommand;
use triggers::{CronCommand, InboxCommand, NewTriggerCommand, TriggersCommand};

// Private helpers the `tests/commands/` mirror reaches through `use super::*`.
#[cfg(test)]
#[allow(unused_imports)]
use misc::help_text;
#[cfg(test)]
#[allow(unused_imports)]
use model::{model_catalog_text, model_groups, unknown_model_error, unknown_provider_error};
#[cfg(test)]
#[allow(unused_imports)]
use skills::parse_skill_source;
#[cfg(test)]
#[allow(unused_imports)]
use triggers::{
    collect_trigger_audit_rows, render_running_triggers, render_trigger_audit,
    render_trigger_sources, trigger_decision_details,
};

pub const THINKING_LEVEL_USAGE: &str = "[off|minimal|low|medium|high|xhigh]";

/// Context handed to a command at runtime — daemon-shaped view kept for the assembly layer
/// (`turn::TurnHost`) and the integration tests: it carries the `trigger_executor` reference.
/// [`dispatch`] converts it into the transport framework's generic
/// [`theway_transport::commands::CommandCtx`]
/// with [`DaemonCtx`] extras; daemon commands (e.g. `/triggers`) reach the executor through
/// `ctx.extra.trigger_executor`.
pub struct CommandCtx<'a> {
    pub harness: &'a Arc<AgentHarness>,
    pub trigger_executor: &'a Arc<TriggerExecutor>,
    pub session_id: &'a str,
    pub log_path: Option<&'a PathBuf>,
    pub tool_count: usize,
    pub cwd: &'a std::path::Path,
}

/// Daemon-only context extras handed to command implementations through the shared
/// framework's generic `CommandCtx::extra` slot.
/// Local commands implement `SlashCommand<X>` for every `X` and ignore it; the daemon
/// runtime commands read the handles they need here instead of reaching for globals.
pub struct DaemonCtx {
    /// Agent runtime used by daemon-owned commands.
    pub harness: Arc<AgentHarness>,
    /// Trigger executor of the running daemon session; `/triggers` reads status
    /// snapshots and aborts running trigger actions through it.
    pub trigger_executor: Arc<TriggerExecutor>,
    /// Runtime storage seam (issue #86): commands open session repos / read
    /// sidecars through this instead of calling `session::open_repo` directly.
    pub storage: Arc<dyn RuntimeStorage>,
    pub dynamic_triggers: crate::triggers::dynamic::DynamicTriggerRegistry,
    pub cron: crate::triggers::cron::CronRegistry,
}

/// Slash-command registry: the shared framework's generic registry parameterized by the daemon's
/// [`DaemonCtx`] extras, plus the daemon assembly entry point
/// ([`Registry::with_daemon_commands`]). Thin wrapper so the daemon can keep its concrete
/// `Registry` type in the assembly layer while the shared framework's constructors only
/// know the local command set.
pub struct Registry {
    inner: theway_transport::commands::Registry<DaemonCtx>,
    /// Claude-code-format file commands scanned from disk (issue #37); the
    /// `/reload` special case in [`dispatch`] rewrites this list.
    file_commands: std::sync::RwLock<Vec<crate::file_commands::FileCommand>>,
    /// User home root for file-command discovery (issue #66): resolved once at
    /// the CLI boundary (`DaemonPaths::home`) and injected via
    /// [`Registry::with_user_home`]; the `/reload` rescan never reads `$HOME`.
    user_home: std::path::PathBuf,
    /// Runtime storage seam (issue #86): wired by the daemon composition root
    /// and handed to command implementations through [`DaemonCtx`]. Defaults
    /// to [`local_runtime_storage`] for tests and embedded hosts.
    storage: Arc<dyn RuntimeStorage>,
    output: CommandOutput,
    dynamic_triggers: crate::triggers::dynamic::DynamicTriggerRegistry,
    cron: crate::triggers::cron::CronRegistry,
}

fn default_dynamic_trigger_registry() -> crate::triggers::dynamic::DynamicTriggerRegistry {
    #[cfg(test)]
    {
        crate::triggers::global_registry().clone()
    }
    #[cfg(not(test))]
    crate::triggers::dynamic::DynamicTriggerRegistry::new()
}

fn default_cron_registry() -> crate::triggers::cron::CronRegistry {
    #[cfg(test)]
    {
        crate::triggers::global_cron_registry().clone()
    }
    #[cfg(not(test))]
    crate::triggers::cron::CronRegistry::new()
}

impl Registry {
    // Part of the pre-split public surface (embedding); no in-tree caller outside
    // `with_daemon_commands`-style assembly.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            inner: theway_transport::commands::Registry::new(),
            file_commands: std::sync::RwLock::new(Vec::new()),
            user_home: std::path::PathBuf::new(),
            storage: local_runtime_storage(),
            output: CommandOutput::default(),
            dynamic_triggers: default_dynamic_trigger_registry(),
            cron: default_cron_registry(),
        }
    }

    /// Full builtin set for the daemon process: the runtime command set
    /// (auth login/logout/sessions, skills/model/goal/session lifecycle/
    /// triggers/…). The TUI keeps its own local set (quit/clear/help).
    pub fn with_daemon_commands() -> Self {
        let mut r = Self {
            inner: theway_transport::commands::Registry::new(),
            file_commands: std::sync::RwLock::new(Vec::new()),
            user_home: std::path::PathBuf::new(),
            storage: local_runtime_storage(),
            output: CommandOutput::default(),
            dynamic_triggers: default_dynamic_trigger_registry(),
            cron: default_cron_registry(),
        };
        r.register(Arc::new(auth::LoginCommand));
        r.register(Arc::new(auth::LogoutCommand));
        r.register(Arc::new(auth::SessionsCommand));
        r.register(Arc::new(SkillsCommand));
        r.register(Arc::new(SkillCommand));
        r.register(Arc::new(ModelCommand));
        r.register(Arc::new(ThinkingCommand));
        r.register(Arc::new(CostCommand));
        r.register(Arc::new(DiagCommand));
        r.register(Arc::new(TemplateCommand));
        r.register(Arc::new(SaveCommand));
        r.register(Arc::new(CompactCommand));
        r.register(Arc::new(UndoCommand));
        r.register(Arc::new(BugReportCommand));
        r.register(Arc::new(NameCommand));
        r.register(Arc::new(ForkCommand));
        r.register(Arc::new(SessionCommand));
        r.register(Arc::new(WebConnectCommand));
        r.register(Arc::new(WebDisconnectCommand));
        r.register(Arc::new(ShareCommand));
        r.register(Arc::new(FindCommand));
        r.register(Arc::new(HistoryCommand));
        r.register(Arc::new(GoalCommand));
        r.register(Arc::new(GoalStartCommand));
        r.register(Arc::new(TriggersCommand));
        r.register(Arc::new(NewTriggerCommand));
        r.register(Arc::new(CronCommand));
        r.register(Arc::new(InboxCommand));
        r
    }

    /// Compatibility alias for [`Registry::with_daemon_commands`], kept for the command
    /// e2e suites.
    pub fn with_builtins() -> Self {
        Self::with_daemon_commands()
    }

    /// Inject the user home root for file-command discovery (issue #66). The
    /// composition root (`bin/thewayd.rs`) wires `DaemonPaths::home` here; the
    /// `/reload` rescan uses it instead of reading `$HOME`.
    #[must_use]
    pub fn with_user_home(mut self, home: std::path::PathBuf) -> Self {
        self.user_home = home;
        self
    }

    /// The user home root file commands are scanned from (see
    /// [`Registry::with_user_home`]).
    pub fn user_home(&self) -> &std::path::Path {
        &self.user_home
    }

    /// Inject the runtime storage seam (issue #86). The composition root
    /// wires the same storage used for session/DAG/trigger/cron state here so
    /// slash commands never open the local repo directly.
    #[must_use]
    pub fn with_storage(mut self, storage: Arc<dyn RuntimeStorage>) -> Self {
        self.storage = storage;
        self
    }

    #[must_use]
    pub fn with_output(mut self, output: CommandOutput) -> Self {
        self.output = output;
        self
    }

    pub fn with_automations(
        mut self,
        dynamic_triggers: crate::triggers::dynamic::DynamicTriggerRegistry,
        cron: crate::triggers::cron::CronRegistry,
    ) -> Self {
        self.dynamic_triggers = dynamic_triggers;
        self.cron = cron;
        self
    }

    pub fn register(&mut self, command: Arc<dyn SlashCommand<DaemonCtx>>) {
        self.inner.register(command);
    }

    /// Replace the scanned claude-code-format file commands (issue #37).
    /// Called at startup by `TurnHost` and on `/reload`.
    pub fn set_file_commands(&self, commands: Vec<crate::file_commands::FileCommand>) {
        *self
            .file_commands
            .write()
            .unwrap_or_else(|e| e.into_inner()) = commands;
    }

    /// Snapshot of the scanned file commands.
    pub fn file_commands(&self) -> Vec<crate::file_commands::FileCommand> {
        self.file_commands
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Slash-prefixed names of the scanned file commands (completion + wire
    /// sidebar surface, issue #37).
    pub fn file_command_names(&self) -> Vec<String> {
        self.file_commands()
            .into_iter()
            .map(|c| format!("/{}", c.name))
            .collect()
    }
}

impl std::ops::Deref for Registry {
    type Target = theway_transport::commands::Registry<DaemonCtx>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_daemon_commands()
    }
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if preview.chars().count() < text.chars().count() {
        preview.push('…');
    }
    preview.replace('\n', " ")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillShortcut {
    pub command: String,
    pub source: SkillSource,
    pub description: String,
}

pub fn skill_shortcuts(skills: &[Skill], registry: &Registry) -> Vec<SkillShortcut> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for skill in skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
    {
        *counts.entry(skill.name.as_str()).or_default() += 1;
    }
    let mut shortcuts = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .filter(|skill| counts.get(skill.name.as_str()) == Some(&1))
        .filter(|skill| registry.find(&skill.name).is_none())
        .map(|skill| SkillShortcut {
            command: format!("/{}", skill.name),
            source: skill.source,
            description: preview_text(&skill.description, 72),
        })
        .collect::<Vec<_>>();
    shortcuts.sort_by(|a, b| a.command.cmp(&b.command));
    shortcuts
}

fn resolve_skill_shortcut<'a>(
    skills: &'a [Skill],
    registry: &Registry,
    name: &str,
) -> Result<Option<&'a Skill>, String> {
    if registry.find(name).is_some() {
        return Ok(None);
    }
    let matching = skills
        .iter()
        .filter(|skill| skill.name == name)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return Ok(None);
    }
    let enabled = matching
        .iter()
        .copied()
        .filter(|skill| !skill.disable_model_invocation)
        .collect::<Vec<_>>();
    match enabled.as_slice() {
        [skill] => Ok(Some(*skill)),
        [] => Err(format!(
            "skill '{name}' is disabled; run /skills enable {name} [source] or /skills to list loaded skills"
        )),
        _ => Err(format!(
            "multiple enabled skills named '{name}'; use /skill {name} after resolving the source with /skills show {name} [source]"
        )),
    }
}

fn run_skill_shortcut(
    name: &str,
    argv: &[String],
    registry: &Registry,
    ctx: &CommandCtx<'_>,
) -> Option<CommandOutcome> {
    match resolve_skill_shortcut(&ctx.harness.skills(), registry, name) {
        Ok(Some(skill)) => {
            cprintln!("using skill: {} ({})", skill.name, skill.source.label());
            if argv.is_empty() {
                Some(CommandOutcome::AttachSkill {
                    name: skill.name.clone(),
                })
            } else {
                Some(CommandOutcome::RunAgentPrompt {
                    prompt: attach_skill_prompt(argv.join(" "), Some(&skill.name)),
                    error_context: "skill command failed: ",
                })
            }
        }
        Ok(None) => None,
        Err(e) => Some(CommandOutcome::Error(e)),
    }
}

pub async fn dispatch(input: &str, registry: &Registry, ctx: &CommandCtx<'_>) -> CommandOutcome {
    registry
        .output
        .scope(dispatch_with_output(input, registry, ctx))
        .await
}

async fn dispatch_with_output(
    input: &str,
    registry: &Registry,
    ctx: &CommandCtx<'_>,
) -> CommandOutcome {
    let (name, argv) = match parse(input) {
        Some(parts) => parts,
        None => return CommandOutcome::Error("not a slash command".into()),
    };
    // Special-case `/help`: the handler can't see the registry, so we render here.
    if name == "help" {
        print_help_with_skills(
            registry,
            argv.first().map(String::as_str),
            &ctx.harness.skills(),
        );
        return CommandOutcome::Handled;
    }
    // Special-case `/reload`: rescan skills + file commands (issue #37).
    if name == "reload" {
        return reload_everything(registry, ctx).await;
    }
    let Some(cmd) = registry.find(&name) else {
        // Claude-code-format file commands take precedence over skill
        // shortcuts of the same name: the file body is an explicit prompt
        // the user wrote for `/name` (issue #37).
        if let Some(file_cmd) = registry
            .file_commands()
            .into_iter()
            .find(|fc| fc.name == name)
        {
            let args_tail = args_tail_of(input, &name);
            return CommandOutcome::RunAgentPrompt {
                prompt: crate::file_commands::expand_file_command(&file_cmd, &args_tail),
                error_context: "",
            };
        }
        return run_skill_shortcut(&name, &argv, registry, ctx).unwrap_or_else(|| {
            // Issue #37: a leading `/` is not necessarily a command — a path
            // like `/etc/hosts` is a plain user message. Send the raw input
            // to the model instead of erroring.
            CommandOutcome::RunAgentPrompt {
                prompt: input.to_string(),
                error_context: "",
            }
        });
    };
    let extra = DaemonCtx {
        harness: ctx.harness.clone(),
        trigger_executor: ctx.trigger_executor.clone(),
        storage: registry.storage.clone(),
        dynamic_triggers: registry.dynamic_triggers.clone(),
        cron: registry.cron.clone(),
    };
    let sdk_ctx = theway_transport::commands::CommandCtx {
        session_id: ctx.session_id,
        log_path: ctx.log_path,
        tool_count: ctx.tool_count,
        cwd: ctx.cwd,
        extra: &extra,
    };
    cmd.run(&argv, &sdk_ctx).await
}

/// The raw argument tail after `/name` in `input` (one leading space
/// stripped, spacing otherwise preserved) — the claude-code `$ARGUMENTS`
/// payload for file commands.
fn args_tail_of(input: &str, name: &str) -> String {
    let trimmed = input.trim();
    let skip = 1 + name.len();
    if trimmed.len() <= skip {
        return String::new();
    }
    trimmed[skip..].trim_start().to_string()
}

/// `/reload` (issue #37): rescan the claude-code-format file commands and
/// hot-reload the skill catalog from disk.
async fn reload_everything(registry: &Registry, ctx: &CommandCtx<'_>) -> CommandOutcome {
    let scanned = crate::file_commands::scan_file_commands(ctx.cwd, registry.user_home());
    let count = scanned.len();
    registry.set_file_commands(scanned.clone());
    cprintln!("reloaded commands: {count} file command(s)");
    for fc in &scanned {
        if fc.description.is_empty() {
            cprintln!("  - /{}", fc.name);
        } else {
            cprintln!("  - /{} — {}", fc.name, fc.description);
        }
    }
    match ctx.harness.reload_skills_from_disk().await {
        Ok(out) => {
            cprintln!(
                "reloaded skills: {} loaded, {} diagnostics",
                out.skills.len(),
                out.diagnostics.len()
            );
        }
        Err(e) => {
            return CommandOutcome::Error(format!("reload skills failed: {e}"));
        }
    }
    CommandOutcome::Handled
}

#[cfg(test)]
// Test files live in `tests/commands/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("commands");
