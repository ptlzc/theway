//! Assorted REPL builtins: `/diag`, `/template`, `/compact`, `/bug-report`,
//! `/web-connect`, `/web-disconnect`, `/find`, `/history`, and the `/help` text builders.
//! (`/help`, `/clear`, `/quit` moved to the SDK local command set — sdk-split-local-sandbox,
//! node 5-commands-layer.)

use super::*;

use super::model::{emit_multiline, model_catalog_text, model_help_summary_lines};
use theway_transport::commands::CommandCtx;

pub struct DiagCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for DiagCommand {
    fn name(&self) -> &'static str {
        "diag"
    }
    fn description(&self) -> &'static str {
        "show diagnostic info (model, thinking, cost, log path)"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let state = ctx.harness.agent().state();
        let model = state
            .model
            .as_ref()
            .map(|m| format!("{}:{}", m.provider.0, m.id))
            .unwrap_or_else(|| "(none)".into());
        let thinking = state
            .thinking_level
            .map(|l| l.as_str())
            .unwrap_or("?")
            .to_string();
        let skill_count = ctx.harness.skills().len();
        let cost = ctx.harness.cost();
        let log = ctx
            .log_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(logging disabled)".into());
        cprintln!();
        cprintln!("Diagnostic snapshot:");
        cprintln!("  session       {}", ctx.session_id);
        cprintln!("  model         {model}");
        cprintln!("  thinking      {thinking}");
        cprintln!("  tools         {}", ctx.tool_count);
        cprintln!("  skills        {skill_count}");
        cprintln!(
            "  cost          {}",
            theway_core::cost_one_line_summary(&cost)
        );
        cprintln!("  log file      {log}");
        cprintln!();
        CommandOutcome::Handled
    }
}

pub struct TemplateCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for TemplateCommand {
    fn name(&self) -> &'static str {
        "template"
    }
    fn description(&self) -> &'static str {
        "list templates, or run one with /template <name> [k=v ...]"
    }
    fn usage(&self) -> &'static str {
        "[name] [k=v ...]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        if argv.is_empty() {
            let templates = ctx.harness.templates();
            if templates.is_empty() {
                cprintln!(
                    "(no templates loaded — drop `.md` files under ~/.theway/templates/ or <cwd>/.theway/templates/)"
                );
            } else {
                cprintln!("Loaded templates ({}):", templates.len());
                for t in &templates {
                    let desc = t.description.clone().unwrap_or_default();
                    cprintln!("  /template {}  {}", t.name, desc);
                }
            }
            return CommandOutcome::Handled;
        }
        let name = argv[0].clone();
        // Remaining args are `k=v` pairs.
        let mut vars = serde_json::Map::new();
        for arg in &argv[1..] {
            if let Some((k, v)) = arg.split_once('=') {
                vars.insert(k.to_string(), serde_json::Value::String(v.to_string()));
            } else {
                return CommandOutcome::Error(format!("expected k=v argument; got: {arg}"));
            }
        }
        CommandOutcome::RunPromptTemplate { name, vars }
    }
}

pub struct CompactCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for CompactCommand {
    fn name(&self) -> &'static str {
        "compact"
    }
    fn description(&self) -> &'static str {
        "force a context compaction now (no-op when nothing to summarize)"
    }
    fn usage(&self) -> &'static str {
        "[\"custom instructions\"]"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let custom = if argv.is_empty() {
            None
        } else {
            Some(argv.join(" "))
        };
        CommandOutcome::RunCompaction { custom }
    }
}

pub struct BugReportCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for BugReportCommand {
    fn name(&self) -> &'static str {
        "bug-report"
    }
    fn description(&self) -> &'static str {
        "write a redacted diagnostic dump for issue attachment"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        // Snapshot the model + thinking with the lock held briefly; the MutexGuard cannot
        // cross an .await so we copy what we need and drop it.
        let (model, thinking) = {
            let state = ctx.harness.agent().state();
            let m = state
                .model
                .as_ref()
                .map(|m| format!("{}:{}", m.provider.0, m.id));
            let t = state
                .thinking_level
                .map(|l| l.as_str())
                .unwrap_or("?")
                .to_string();
            (m, t)
        };
        let cost = ctx.harness.cost();
        let diag = crate::bug_report::DiagInputs {
            session_id: ctx.session_id.to_string(),
            model,
            thinking,
            tool_count: ctx.tool_count,
            skill_count: ctx.harness.skills().len(),
            cost_summary: theway_core::cost_one_line_summary(&cost),
            log_path: ctx.log_path.cloned(),
        };
        let dest = crate::bug_report::default_dest();
        match crate::bug_report::build(diag, ctx.harness.session(), &dest).await {
            Ok(path) => {
                cprintln!("wrote bug report: {}", path.display());
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("bug-report failed: {e}")),
        }
    }
}

pub struct WebConnectCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for WebConnectCommand {
    fn name(&self) -> &'static str {
        "web-connect"
    }
    fn description(&self) -> &'static str {
        "mount this session at the public relay (watch + prompt via secret URL)"
    }
    fn usage(&self) -> &'static str {
        "[status]"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        match argv.first().map(String::as_str) {
            None => CommandOutcome::WebRelay(WebRelayAction::Connect),
            Some("status") => CommandOutcome::WebRelay(WebRelayAction::Status),
            Some(other) => CommandOutcome::Error(format!("unknown /web-connect argument: {other}")),
        }
    }
}

pub struct WebDisconnectCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for WebDisconnectCommand {
    fn name(&self) -> &'static str {
        "web-disconnect"
    }
    fn description(&self) -> &'static str {
        "disconnect the public relay and invalidate the session URL"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        CommandOutcome::WebRelay(WebRelayAction::Disconnect)
    }
}

pub struct FindCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for FindCommand {
    fn name(&self) -> &'static str {
        "find"
    }
    fn description(&self) -> &'static str {
        "search every session in this cwd for prompts/replies containing <query>"
    }
    fn usage(&self) -> &'static str {
        "<query>"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        if argv.is_empty() {
            return CommandOutcome::Error("usage: /find <query>".into());
        }
        let query = argv.join(" ").to_lowercase();
        let repo = theway_storage::session::open_repo(ctx.cwd).await;
        let files = match repo.list().await {
            Ok(f) => f,
            Err(e) => return CommandOutcome::Error(format!("list sessions: {e}")),
        };
        let mut hits = 0usize;
        for path in files {
            let session = match repo.open(&path).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let entries = session.entries().await.unwrap_or_default();
            for e in entries {
                if let theway_core::SessionTreeEntry::Message { message, .. } = e {
                    let text = match &message {
                        theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(u)) => {
                            match &u.content {
                                theway_llm_provider::UserContent::Text(s) => s.clone(),
                                theway_llm_provider::UserContent::Blocks(blocks) => blocks
                                    .iter()
                                    .filter_map(|b| match b {
                                        theway_llm_provider::UserContentBlock::Text(t) => {
                                            Some(t.text.clone())
                                        }
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            }
                        }
                        theway_core::AgentMessage::Llm(
                            theway_llm_provider::Message::Assistant(a),
                        ) => a
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                theway_llm_provider::ContentBlock::Text(t) => Some(t.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join(" "),
                        _ => continue,
                    };
                    if text.to_lowercase().contains(&query) {
                        hits += 1;
                        let snip = text
                            .chars()
                            .take(120)
                            .collect::<String>()
                            .replace('\n', " ");
                        let path_short = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                        cprintln!("  {path_short}  {snip}");
                    }
                }
            }
        }
        if hits == 0 {
            cprintln!("(no matches)");
        } else {
            cprintln!("({hits} match(es))");
        }
        CommandOutcome::Handled
    }
}

pub struct HistoryCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for HistoryCommand {
    fn name(&self) -> &'static str {
        "history"
    }
    fn description(&self) -> &'static str {
        "show recent submitted prompts from ~/.theway/history"
    }
    fn usage(&self) -> &'static str {
        "[N]"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let limit: usize = argv.first().and_then(|s| s.parse().ok()).unwrap_or(20);
        let store = theway_transport::history::HistoryStore::load();
        let entries = store.entries();
        if entries.is_empty() {
            cprintln!("(no history yet)");
            return CommandOutcome::Handled;
        }
        let start = entries.len().saturating_sub(limit);
        for (i, e) in entries[start..].iter().enumerate() {
            let n = start + i + 1;
            // Truncate long entries to 200 chars to keep the listing skimmable.
            let preview: String = e.chars().take(200).collect();
            let suffix = if preview.len() < e.len() { "…" } else { "" };
            cprintln!("  {n}: {preview}{suffix}");
        }
        CommandOutcome::Handled
    }
}

#[allow(dead_code)]
pub fn print_help(registry: &Registry, topic: Option<&str>) {
    emit_multiline(&help_text_with_skills(registry, topic, &[]));
}

pub fn print_help_with_skills(registry: &Registry, topic: Option<&str>, skills: &[Skill]) {
    emit_multiline(&help_text_with_skills(registry, topic, skills));
}

#[allow(dead_code)]
pub(super) fn help_text(registry: &Registry, topic: Option<&str>) -> String {
    help_text_with_skills(registry, topic, &[])
}

fn help_text_with_skills(registry: &Registry, topic: Option<&str>, skills: &[Skill]) -> String {
    let Some(topic) = topic.map(str::trim).filter(|topic| !topic.is_empty()) else {
        return general_help_text(registry, skills);
    };
    let topic = topic.trim_start_matches('/');
    if topic == "models" {
        return model_catalog_text(None).unwrap_or_else(|e| e);
    }

    command_help_text(registry, topic, skills)
}

fn general_help_text(registry: &Registry, skills: &[Skill]) -> String {
    let mut lines = vec![String::new(), "Commands:".into()];
    for cmd in registry.commands() {
        let aliases = if cmd.aliases().is_empty() {
            String::new()
        } else {
            format!(" (aliases: {})", cmd.aliases().join(", "))
        };
        let usage = if cmd.usage().is_empty() {
            String::new()
        } else {
            format!(" {}", cmd.usage())
        };
        lines.push(format!(
            "  /{}{}    {}{}",
            cmd.name(),
            usage,
            cmd.description(),
            aliases
        ));
    }
    let shortcuts = skill_shortcuts(skills, registry);
    if !shortcuts.is_empty() {
        lines.push(String::new());
        lines.push("Skill commands:".into());
        for shortcut in shortcuts {
            let description = if shortcut.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", shortcut.description)
            };
            lines.push(format!(
                "  {} [prompt]    use loaded skill ({}){}",
                shortcut.command,
                shortcut.source.label(),
                description
            ));
        }
    }
    lines.push(String::new());
    lines.push("Models:".into());
    for line in model_help_summary_lines() {
        lines.push(line);
    }
    lines.push(String::new());
    lines.push("Anything else is sent as a prompt to the agent.".into());
    lines.push(String::new());
    lines.join("\n")
}

fn command_help_text(registry: &Registry, topic: &str, skills: &[Skill]) -> String {
    let Some(cmd) = registry.find(topic) else {
        if let Ok(Some(skill)) = resolve_skill_shortcut(skills, registry, topic) {
            let mut lines = vec![
                format!("/{topic} [prompt]"),
                format!(
                    "  use loaded skill '{}' ({})",
                    skill.name,
                    skill.source.label()
                ),
            ];
            if !skill.description.is_empty() {
                lines.push(format!("  {}", preview_text(&skill.description, 120)));
            }
            lines.push(format!("  equivalent: /skill {}", skill.name));
            return lines.join("\n");
        }
        let suggestions = registry
            .commands()
            .iter()
            .filter(|cmd| cmd.name().starts_with(topic) || cmd.aliases().contains(&topic))
            .map(|cmd| format!("/{}", cmd.name()))
            .chain(
                skill_shortcuts(skills, registry)
                    .into_iter()
                    .filter(|shortcut| shortcut.command[1..].starts_with(topic))
                    .map(|shortcut| shortcut.command),
            )
            .take(5)
            .collect::<Vec<_>>();
        let suggestion = if suggestions.is_empty() {
            "Run /help to list commands or /help models for the model catalog.".to_string()
        } else {
            format!("Did you mean {}?", suggestions.join(", "))
        };
        return format!("unknown help topic: {topic}\n{suggestion}");
    };

    let usage = if cmd.usage().is_empty() {
        format!("/{}", cmd.name())
    } else {
        format!("/{} {}", cmd.name(), cmd.usage())
    };
    let mut lines = vec![usage, format!("  {}", cmd.description())];
    if !cmd.aliases().is_empty() {
        let aliases = cmd
            .aliases()
            .iter()
            .map(|alias| format!("/{alias}"))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("  aliases: {aliases}"));
    }
    if cmd.name() == "help" {
        lines.push("  examples: /help model, /help /quit, /help models".into());
    } else {
        lines.push(format!("  more: /help {}", cmd.name()));
    }
    lines.join("\n")
}
