//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Slash-command framework: [`CommandOutcome`], [`CommandCtx`], the [`SlashCommand`]
//! trait, [`Registry`], the shared output sink ([`console`]), slash parsing ([`parse`]),
//! and the pure helpers shared by command implementations and the CLI layer
//! (`parse_model_spec`, `attach_skill_prompt`, …).
//!
//! The command table is shared contract — the TUI (completion) and the daemon
//! (execution) both program against it. The framework is generic over the context
//! extras (`CommandCtx<'a, X>` /
//! `SlashCommand<X>`): the daemon layers its runtime commands on top with its own
//! extras type (`DaemonCtx`); the TUI registers its local UI commands. Command
//! implementations themselves live with their owners (tui / daemon).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use theway_core::AgentHarness;
use theway_llm_provider::Model;

/// Sink for slash-command output. The full-screen TUI owns the only terminal writer, so
/// commands must not `println!` straight to stdout — they route through here. The app installs
/// a sink that forwards each line into the conversation feed; when none is installed (unit
/// tests, non-interactive shells) output falls back to stdout.
///
/// This is the single process-wide sink: daemon-side command implementations route
/// here too, so local and daemon command output share one surface.
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
    /// Not `cfg(test)`-gated: integration tests that path-include the command modules compile
    /// this crate as a dependency (where `cfg(test)` is inactive) and still need this.
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

/// Drop-in replacement for `println!` in command implementations: same call syntax, but the
/// formatted line is routed through [`console::emit_line`] instead of straight to stdout.
/// Exported at the crate root so command implementations in any crate (the daemon's
/// runtime commands, the TUI's local commands) can share it.
#[macro_export]
macro_rules! cprintln {
    () => { $crate::commands::console::emit_line(String::new()) };
    ($($arg:tt)*) => { $crate::commands::console::emit_line(std::format!($($arg)*)) };
}

/// Thinking levels accepted by `/thinking` and the `--thinking` CLI flag.
pub const THINKING_LEVEL_VALUES: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

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
/// explicit. The `extra` field carries execution-environment extras (generic so the daemon
/// can layer its runtime context in node 6 without changing local commands, which implement
/// `SlashCommand<X>` for every `X`).
pub struct CommandCtx<'a, X = ()> {
    pub harness: &'a Arc<AgentHarness>,
    pub session_id: &'a str,
    pub log_path: Option<&'a PathBuf>,
    pub tool_count: usize,
    pub cwd: &'a std::path::Path,
    pub extra: &'a X,
}

#[async_trait]
pub trait SlashCommand<X = ()>: Send + Sync {
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
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, X>) -> CommandOutcome;
}

/// In-memory registry. Lookups are linear scans over a small set — `O(n)` is fine.
pub struct Registry<X = ()> {
    commands: Vec<Arc<dyn SlashCommand<X>>>,
}

impl<X> Registry<X> {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    pub fn register(&mut self, command: Arc<dyn SlashCommand<X>>) {
        self.commands.push(command);
    }

    pub fn commands(&self) -> &[Arc<dyn SlashCommand<X>>] {
        &self.commands
    }

    /// Lookup by name or alias. `name` is the bare command without `/`.
    pub fn find(&self, name: &str) -> Option<Arc<dyn SlashCommand<X>>> {
        self.commands
            .iter()
            .find(|c| c.name() == name || c.aliases().contains(&name))
            .cloned()
    }

    /// Every registered name and alias (completion surface for clients).
    pub fn names(&self) -> Vec<&'static str> {
        self.commands
            .iter()
            .flat_map(|c| std::iter::once(c.name()).chain(c.aliases().iter().copied()))
            .collect()
    }
}

impl<X> Default for Registry<X> {
    fn default() -> Self {
        Self::new()
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

// ---------------------------------------------------------------------------
// Pure helpers shared by command implementations and the CLI layer.
// ---------------------------------------------------------------------------

/// Parse a `provider:model-id` / `provider/model-id` / `provider model-id` spec into its
/// two parts. Returns `None` when either part is missing.
pub fn parse_model_spec(spec: &str) -> Option<(&str, &str)> {
    let spec = spec.trim();
    let (provider, id) = spec
        .split_once(':')
        .or_else(|| spec.split_once('/'))
        .or_else(|| spec.split_once(char::is_whitespace))?;
    let provider = provider.trim();
    let id = id.trim();
    if provider.is_empty() || id.is_empty() {
        return None;
    }
    Some((provider, id))
}

/// Wrap a user prompt so the agent invokes the named skill first. Without a skill name the
/// text passes through unchanged (never embeds the skill body — the agent loads it via the
/// Skill tool).
pub fn attach_skill_prompt(text: impl Into<String>, skill_name: Option<&str>) -> String {
    let text = text.into();
    let Some(skill_name) = skill_name else {
        return text;
    };
    format!(
        "Before answering, invoke the Skill tool with name \"{skill_name}\" and use that skill's instructions for this turn.\n\nUser request:\n{text}"
    )
}

/// Grouped model catalog used by the help/model text builders: provider -> models sorted
/// by id.
pub fn model_groups() -> BTreeMap<String, Vec<Model>> {
    let mut groups: BTreeMap<String, Vec<Model>> = BTreeMap::new();
    for model in theway_llm_provider::list_models() {
        groups
            .entry(model.provider.0.clone())
            .or_default()
            .push(model);
    }
    for models in groups.values_mut() {
        models.sort_by(|a, b| a.id.cmp(&b.id));
    }
    groups
}

/// One-line `provider(count)` summary, e.g. `anthropic(12), openai(8)`.
pub fn provider_summary(groups: &BTreeMap<String, Vec<Model>>) -> String {
    groups
        .iter()
        .map(|(provider, models)| format!("{provider}({})", models.len()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Summary lines for the model catalog section of `/help` (provider/model counts, custom
/// model locations, credential hint). No secrets: counts and names only.
pub fn model_help_summary_lines() -> Vec<String> {
    let groups = model_groups();
    let total = groups.values().map(Vec::len).sum::<usize>();
    vec![
        format!(
            "  Supported providers ({}), models ({}): {}",
            groups.len(),
            total,
            provider_summary(&groups)
        ),
        "  Full list: /help models or /model list [provider]".into(),
        "  Custom models: ~/.theway/models.json and <cwd>/.theway/models.json".into(),
        "  Credentials: set provider env vars or run /login <provider>.".into(),
    ]
}

/// `--help` epilogue text listing the supported providers and custom-model locations.
pub fn cli_model_help_text() -> String {
    let mut out = String::new();
    out.push_str("Model catalog:\n");
    for line in model_help_summary_lines() {
        out.push_str("  ");
        out.push_str(line.trim_start());
        out.push('\n');
    }
    out
}
