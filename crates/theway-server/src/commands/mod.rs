//! Slash-command registry — daemon layer.
//!
//! The command framework (Registry / SlashCommand / CommandOutcome / CommandCtx, console
//! sink, `parse`, pure helpers) and the local command set (quit/clear/help/login/logout/
//! sessions) moved to `theway-sdk` (sdk-split-local-sandbox, node 5-commands-layer); this
//! module keeps the daemon-side surface:
//!
//! - re-exports of the SDK framework so existing `crate::commands::…` / `theway::commands::…`
//!   paths keep resolving (TUI, readline, tests);
//! - the daemon command implementations in submodules: [`skills`] / [`skill_cmd`] (skill
//!   management), [`model`] (`/model`, `/thinking`, `/cost` + model-catalog help), [`goal`]
//!   (`/goal`, `/goal-start`), [`session`] (session lifecycle), [`triggers`] (automation),
//!   and [`misc`] (everything else);
//! - the compatibility [`Registry`] wrapper whose `with_builtins()` assembles the full set
//!   (SDK local commands + daemon commands) — node 6 replaces it with
//!   `with_daemon_commands` over a `DaemonCtx` extras type;
//! - [`dispatch`], which converts the daemon-shaped [`CommandCtx`] into the SDK's generic
//!   context and routes `/help` + skill shortcuts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::execution::{NotificationStatusSnapshot, RunningTriggerState};
use crate::trigger_engine::notification_hook::{HookState, NotificationHookStatus};
use async_trait::async_trait;
use serde_json::json;
use theway_core::{AgentHarness, AgentTool, SessionTreeEntry, Skill, SkillSource, ThinkingLevel};
use theway_llm_provider::{Model, Provider, UserContentBlock, get_model};
use tokio_util::sync::CancellationToken;

// Framework moved to the SDK (node 5-commands-layer). Re-exported so existing
// `commands::…` call sites (ui, main, readline, model_picker, tests) keep their paths.
// The allow keeps the pre-split `commands::…` paths alive in the path-included e2e test
// crate, where this module is private and some re-exports have no in-tree user.
/// The SDK's console sink is the single process-wide output sink: daemon commands route
//  through it too (see the `cprintln!` macro below).
pub use theway_sdk::commands::console;
#[allow(unused_imports)]
pub use theway_sdk::commands::{
    CommandOutcome, SlashCommand, THINKING_LEVEL_VALUES, WebRelayAction, attach_skill_prompt,
    cli_model_help_text, model_credential_hint, parse, parse_model_spec, save_api_key,
};

/// Drop-in replacement for `println!` inside this module: same call syntax, but the formatted
/// line is routed through the (SDK-owned, process-wide) [`console::emit_line`] instead of
/// straight to stdout. Defined before the command submodules so they see it.
macro_rules! cprintln {
    () => { $crate::commands::console::emit_line(String::new()) };
    ($($arg:tt)*) => { $crate::commands::console::emit_line(std::format!($($arg)*)) };
}

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

// Command implementations registered by `Registry::with_builtins` (the local set comes from
// `theway_sdk::commands::Registry::local()`).
use goal::{GoalCommand, GoalStartCommand};
use misc::{
    BugReportCommand, CompactCommand, DiagCommand, FindCommand, HistoryCommand, TemplateCommand,
    WebConnectCommand, WebDisconnectCommand,
};
use model::{CostCommand, ModelCommand, ThinkingCommand};
use session::{NameCommand, SaveCommand, SessionCommand, ShareCommand, UndoCommand};
use skill_cmd::SkillCommand;
use skills::SkillsCommand;
use triggers::{CronCommand, InboxCommand, NewTriggerCommand, TriggersCommand};

// Private helpers the `tests/commands/` mirror reaches through `use super::*`.
#[cfg(test)]
use misc::help_text;
#[cfg(test)]
use model::{model_catalog_text, model_groups, unknown_model_error, unknown_provider_error};
#[cfg(test)]
use skills::parse_skill_source;
#[cfg(test)]
use triggers::{
    collect_trigger_audit_rows, render_running_triggers, render_trigger_audit,
    render_trigger_sources, trigger_decision_details,
};

pub const THINKING_LEVEL_USAGE: &str = "[off|minimal|low|medium|high|xhigh]";

/// Context handed to a command at runtime — daemon-shaped view kept for the assembly layer
/// (`DaemonApp`) and the integration tests: it still carries the `trigger_executor`
/// reference. [`dispatch`] converts it into the SDK's generic [`theway_sdk::commands::CommandCtx`]
/// (extras `()`); `/triggers` reaches the executor through [`current_trigger_executor`].
/// Node 6 replaces this with a proper `DaemonCtx` extras type.
pub struct CommandCtx<'a> {
    pub harness: &'a Arc<AgentHarness>,
    pub trigger_executor: &'a Arc<TriggerExecutor>,
    pub session_id: &'a str,
    pub log_path: Option<&'a PathBuf>,
    pub tool_count: usize,
    pub cwd: &'a std::path::Path,
}

// Node-5 compat bridge: the SDK framework's `CommandCtx<'a, ()>` carries no trigger
// executor, but `/triggers` still needs one. `dispatch` stashes the daemon's executor here
// right before running a command; node 6 replaces this slot with the `DaemonCtx` extras.
static TRIGGER_EXECUTOR_SLOT: parking_lot::Mutex<Option<Arc<TriggerExecutor>>> =
    parking_lot::Mutex::new(None);

pub(crate) fn install_trigger_executor(executor: Arc<TriggerExecutor>) {
    *TRIGGER_EXECUTOR_SLOT.lock() = Some(executor);
}

pub(crate) fn current_trigger_executor() -> Option<Arc<TriggerExecutor>> {
    TRIGGER_EXECUTOR_SLOT.lock().clone()
}

/// Slash-command registry: the SDK's generic [`theway_sdk::commands::Registry<()>`] plus the
/// daemon assembly entry point. Thin wrapper so `Registry::with_builtins()` can keep
/// returning the *full* builtin set (local + daemon commands) while the SDK's inherent
/// constructors only know the local set.
pub struct Registry {
    inner: theway_sdk::commands::Registry<()>,
}

impl Registry {
    // Part of the pre-split public surface (embedding); no in-tree caller outside
    // `with_builtins`-style assembly.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            inner: theway_sdk::commands::Registry::new(),
        }
    }

    /// Full builtin set: SDK local commands (`Registry::local()`) plus the daemon runtime
    /// commands. Compatibility assembly — node 6 splits this into
    /// `with_daemon_commands()` over `DaemonCtx`.
    pub fn with_builtins() -> Self {
        let mut r = Self {
            inner: theway_sdk::commands::Registry::local(),
        };
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

    pub fn register(&mut self, command: Arc<dyn SlashCommand<()>>) {
        self.inner.register(command);
    }
}

impl std::ops::Deref for Registry {
    type Target = theway_sdk::commands::Registry<()>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
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
    // Compat bridge for `/triggers` (see TRIGGER_EXECUTOR_SLOT); node 6 introduces DaemonCtx.
    install_trigger_executor(ctx.trigger_executor.clone());
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
    let Some(cmd) = registry.find(&name) else {
        return run_skill_shortcut(&name, &argv, registry, ctx).unwrap_or_else(|| {
            CommandOutcome::Error(format!("unknown command: /{name} (try /help)"))
        });
    };
    let sdk_ctx = theway_sdk::commands::CommandCtx {
        harness: ctx.harness,
        session_id: ctx.session_id,
        log_path: ctx.log_path,
        tool_count: ctx.tool_count,
        cwd: ctx.cwd,
        extra: &(),
    };
    cmd.run(&argv, &sdk_ctx).await
}

#[cfg(test)]
// Test files live in `tests/commands/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("commands");
