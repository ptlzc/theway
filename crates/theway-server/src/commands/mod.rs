//! Slash-command registry. Tracks a small set of REPL builtins and dispatches by name.
//!
//! Built-in commands today: `/help`, `/clear`, `/skills`, `/skill`, `/quit` (and aliases),
//! `/model`, `/thinking`. The trait is shaped so future extensions (issue #10 Part B) can
//! register additional commands without touching this file.
//!
//! Command families live in submodules: [`skills`] / [`skill_cmd`] (skill management),
//! [`model`] (`/model`, `/thinking`, `/cost` + model-catalog help), [`goal`] (`/goal`,
//! `/goal-start`), [`session`] (session lifecycle), [`triggers`] (automation), and
//! [`misc`] (everything else). This module keeps the shared surface: the output sink,
//! [`CommandOutcome`], [`CommandCtx`], the [`SlashCommand`] trait, [`Registry`], [`parse`],
//! and [`dispatch`].

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::trigger_engine::execution::TriggerExecutor;
use crate::trigger_engine::execution::{NotificationStatusSnapshot, RunningTriggerState};
use crate::trigger_engine::notification_hook::{HookState, NotificationHookStatus};
use async_trait::async_trait;
use serde_json::json;
use theway_core::{AgentHarness, AgentTool, SessionTreeEntry, Skill, SkillSource, ThinkingLevel};
use theway_llm_provider::{Model, Provider, UserContentBlock, get_model, list_models};
use tokio_util::sync::CancellationToken;

/// Sink for slash-command output. The full-screen TUI owns the only terminal writer, so
/// commands must not `println!` straight to stdout — they route through here. The app installs
/// a sink that forwards each line into the conversation feed; when none is installed (unit
/// tests, non-interactive shells) output falls back to stdout.
pub mod console {
    use parking_lot::Mutex;

    type Sink = Box<dyn Fn(String) + Send + Sync>;
    static SINK: Mutex<Option<Sink>> = Mutex::new(None);

    /// Install the line sink. Called once by the UI at startup. Unused when `commands.rs` is
    /// path-included by integration tests (which never install a sink).
    #[cfg_attr(test, allow(dead_code))]
    pub fn set_sink(sink: Sink) {
        *SINK.lock() = Some(sink);
    }

    /// Clear the active line sink. Used by tests to avoid leaking capture sinks across cases.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn clear_sink() {
        *SINK.lock() = None;
    }

    /// Emit one line of command output through the active sink (or stdout when unset).
    pub fn emit_line(line: String) {
        match SINK.lock().as_ref() {
            Some(sink) => sink(line),
            None => println!("{line}"),
        }
    }
}

/// Drop-in replacement for `println!` inside this module: same call syntax, but the formatted
/// line is routed through [`console::emit_line`] instead of straight to stdout.
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
pub use model::{cli_model_help_text, model_credential_hint, parse_model_spec};
// Only `tests/commands.rs` calls through these `commands::…` paths; the allow keeps the
// pre-split pub(crate) surface without tripping unused_imports in non-test builds.
pub use session::save_api_key;
pub use skill_cmd::attach_skill_prompt;
#[allow(unused_imports)]
pub(crate) use triggers::{render_cron_jobs, render_dynamic_trigger_rules, render_triggers_status};

// Command implementations registered by `Registry::with_builtins`.
use goal::{GoalCommand, GoalStartCommand};
use misc::{
    BugReportCommand, ClearCommand, CompactCommand, DiagCommand, FindCommand, HelpCommand,
    HistoryCommand, QuitCommand, TemplateCommand, WebConnectCommand, WebDisconnectCommand,
};
use model::{CostCommand, ModelCommand, ThinkingCommand};
use session::{
    LoginCommand, LogoutCommand, NameCommand, SaveCommand, SessionCommand, SessionsCommand,
    ShareCommand, UndoCommand,
};
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

#[cfg_attr(test, allow(dead_code))]
pub const THINKING_LEVEL_VALUES: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];
pub const THINKING_LEVEL_USAGE: &str = "[off|minimal|low|medium|high|xhigh]";

/// Outcome of running a command. Drives the REPL's next action.
#[cfg_attr(test, allow(dead_code))]
pub enum CommandOutcome {
    /// Continue the REPL loop normally.
    Handled,
    /// Quit the REPL cleanly.
    Quit,
    /// Clear the screen — REPL handles the ANSI escape so we don't bake it into commands.
    ClearScreen,
    /// Command surfaced an error message; REPL renders it via `tui.error_line`.
    Error(String),
    /// Attach the named skill to the next user prompt. The REPL owns prompt assembly, so this
    /// stays explicit instead of going through the agent steering queue.
    AttachSkill { name: String },
    /// Ask the REPL to run a prompt through the same active-turn path as normal user input.
    /// Commands return this instead of awaiting the harness directly so Ctrl-C/Esc can abort
    /// thinking, streaming, and tool execution consistently.
    RunAgentPrompt {
        prompt: String,
        error_context: &'static str,
    },
    /// Ask the REPL to render and run a prompt template through the active-turn path.
    RunPromptTemplate {
        name: String,
        vars: serde_json::Map<String, serde_json::Value>,
    },
    /// Ask the REPL to run compaction through the active-turn path so Ctrl-C/Esc can abort
    /// the model summarization request.
    RunCompaction { custom: Option<String> },
    /// Prompt for a credential without echoing the secret in the terminal input line.
    ///
    /// `provider` is the user-facing label used in prompts. `storage_key` is the optional auth
    /// store key when the internal lookup key must not be echoed back to the user.
    LoginSecret {
        provider: String,
        storage_key: Option<String>,
        recovery_command: Option<String>,
    },
    /// Bare `/model` — the REPL owns the interactive picker UI, so the
    /// command requests it instead of printing the catalog.
    OpenModelPicker,
    /// `/web-connect` family — the relay lives on the UI `App`, so the REPL layer
    /// performs the action (issue #22).
    WebRelay(WebRelayAction),
    /// A `/session import` brought disabled automation along; ask the user (via the
    /// shared confirm surface) whether to re-enable what the source had enabled.
    SessionImportActivation {
        session_path: std::path::PathBuf,
        trigger_ids: Vec<String>,
        cron_ids: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebRelayAction {
    Connect,
    Status,
    Disconnect,
}

impl std::fmt::Debug for CommandOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Handled => f.write_str("Handled"),
            Self::Quit => f.write_str("Quit"),
            Self::ClearScreen => f.write_str("ClearScreen"),
            Self::Error(message) => f.debug_tuple("Error").field(message).finish(),
            Self::AttachSkill { name } => {
                f.debug_struct("AttachSkill").field("name", name).finish()
            }
            Self::RunAgentPrompt {
                prompt,
                error_context,
            } => f
                .debug_struct("RunAgentPrompt")
                .field("prompt", prompt)
                .field("error_context", error_context)
                .finish(),
            Self::RunPromptTemplate { name, vars } => f
                .debug_struct("RunPromptTemplate")
                .field("name", name)
                .field("vars", vars)
                .finish(),
            Self::RunCompaction { custom } => f
                .debug_struct("RunCompaction")
                .field("custom", custom)
                .finish(),
            Self::LoginSecret {
                provider,
                storage_key,
                recovery_command,
            } => f
                .debug_struct("LoginSecret")
                .field("provider", provider)
                .field("storage_key", storage_key)
                .field("recovery_command", recovery_command)
                .finish(),
            Self::OpenModelPicker => f.write_str("OpenModelPicker"),
            Self::WebRelay(action) => f.debug_tuple("WebRelay").field(action).finish(),
            Self::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => f
                .debug_struct("SessionImportActivation")
                .field("session_path", session_path)
                .field("trigger_ids", &trigger_ids.len())
                .field("cron_ids", &cron_ids.len())
                .finish(),
        }
    }
}

/// Context handed to a command at runtime. Kept narrow so each command's dependencies are
/// explicit.
pub struct CommandCtx<'a> {
    pub harness: &'a Arc<AgentHarness>,
    pub trigger_executor: &'a Arc<TriggerExecutor>,
    pub session_id: &'a str,
    pub log_path: Option<&'a PathBuf>,
    pub tool_count: usize,
    pub cwd: &'a std::path::Path,
}

#[async_trait]
pub trait SlashCommand: Send + Sync {
    /// Canonical name without the leading `/`.
    fn name(&self) -> &'static str;
    /// Optional aliases (also without leading `/`).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    fn description(&self) -> &'static str;
    /// Optional argument hint shown in `/help`. Empty when the command takes no arguments.
    fn usage(&self) -> &'static str {
        ""
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_>) -> CommandOutcome;
}

/// In-memory registry. Lookups are linear scans over a small set — `O(n)` is fine.
pub struct Registry {
    commands: Vec<Arc<dyn SlashCommand>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register(Arc::new(HelpCommand));
        r.register(Arc::new(ClearCommand));
        r.register(Arc::new(SkillsCommand));
        r.register(Arc::new(SkillCommand));
        r.register(Arc::new(QuitCommand));
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
        r.register(Arc::new(SessionsCommand));
        r.register(Arc::new(ShareCommand));
        r.register(Arc::new(LoginCommand));
        r.register(Arc::new(LogoutCommand));
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

    pub fn register(&mut self, command: Arc<dyn SlashCommand>) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[Arc<dyn SlashCommand>] {
        &self.commands
    }

    /// Lookup by name or alias. `name` is the bare command without `/`.
    pub fn find(&self, name: &str) -> Option<Arc<dyn SlashCommand>> {
        self.commands
            .iter()
            .find(|c| c.name() == name || c.aliases().contains(&name))
            .cloned()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Split `/cmd arg1 "arg with spaces"` into `(cmd, [arg1, arg with spaces])`. Returns `None`
/// if `input` doesn't start with `/`. Quoting is minimal: balanced double quotes only.
pub fn parse(input: &str) -> Option<(String, Vec<String>)> {
    let trimmed = input.trim_start();
    let body = trimmed.strip_prefix('/')?;
    let mut argv: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in body.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                }
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        argv.push(current);
    }
    if argv.is_empty() {
        // Bare `/` — no command name.
        return None;
    }
    let name = argv.remove(0);
    Some((name, argv))
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
    cmd.run(&argv, ctx).await
}

#[cfg(test)]
// Test files live in `tests/commands/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/RUST_TEST_FILES.md.
tests_bridge_macro::tests_bridge!("commands");
