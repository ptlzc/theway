//! CLI argument types and standalone session commands for the `theway` binary.
//!
//! Split out of `main.rs` (which keeps the `fn main` entry point and the
//! `run_cli_command` / `run_session_cli_command` dispatch). Mechanical module
//! extraction — behavior is unchanged.

use std::io::IsTerminal as _;

use crate::resume_picker;
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use theway::{commands, config, session, session_archive};
use theway_core::JsonlSessionRepo;

#[derive(Parser, Debug)]
#[command(
    name = "theway",
    version,
    about = "Simple coding agent on top of theway-core"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<CliCommand>,

    /// Provider id (anthropic, openai, openrouter, …). When unset, auto-detected from env.
    #[arg(long)]
    pub(crate) provider: Option<String>,
    /// Model id within the provider's catalog.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Override the selected model's base URL for this run. Useful for local OpenAI-compatible
    /// servers such as DS4.
    #[arg(long = "base-url", value_name = "URL")]
    pub(crate) base_url: Option<String>,
    /// Thinking level (off | minimal | low | medium | high | xhigh).
    #[arg(
        long,
        default_value = "off",
        value_parser = clap::builder::PossibleValuesParser::new(commands::THINKING_LEVEL_VALUES)
    )]
    pub(crate) thinking: String,

    /// Select a session for this cwd to resume. Pass an id to resume a specific one
    /// directly (same as --resume-id); bare --resume opens the picker.
    #[arg(long, value_name = "ID", num_args = 0..=1)]
    pub(crate) resume: Option<Option<String>>,
    /// Continue the most recent session for this cwd.
    #[arg(long = "continue", short = 'c')]
    pub(crate) continue_: bool,
    /// Resume a specific session by id (full UUIDv7 or a unique prefix).
    #[arg(long, value_name = "ID")]
    pub(crate) resume_id: Option<String>,

    /// List sessions for this cwd and exit.
    #[arg(long)]
    pub(crate) list_sessions: bool,
    /// List sessions across every cwd we know about (~/.theway/sessions/*) and exit.
    #[arg(long)]
    pub(crate) list_all_sessions: bool,
    /// Delete a session by id and exit.
    #[arg(long, value_name = "ID")]
    pub(crate) delete_session: Option<String>,
    /// Attach an image to the first prompt of this session. Repeatable. Supported formats:
    /// PNG, JPEG, WebP, GIF. Each image is capped at 10 MiB; max 10 per message.
    #[arg(long = "image", value_name = "PATH")]
    pub(crate) image: Vec<std::path::PathBuf>,

    /// Enable a built-in skill bundled with this `theway` binary, by name. Repeatable. Unknown
    /// names hard-fail with a list of available built-ins. Built-in skills are the lowest
    /// precedence — user (`~/.theway/skills/`) and project (`<cwd>/.theway/skills/`) skills of the
    /// same name still override. Persistent enable is via `~/.theway/config.toml`
    /// `[builtin_skills] enabled = [...]`; CLI + config are unioned and de-duplicated.
    #[arg(long = "builtin-skill", value_name = "NAME")]
    pub(crate) builtin_skill: Vec<String>,

    /// Poll interval for local dynamic trigger checks, in seconds. Defaults to
    /// `[triggers] poll_interval_secs` from `~/.theway/config.toml`, or 600 when unset.
    #[arg(long = "trigger-poll-secs", value_name = "SECONDS", value_parser = clap::value_parser!(u64).range(1..))]
    pub(crate) trigger_poll_secs: Option<u64>,

    /// Show LLM call debug logs in the conversation feed, including trigger/sub-agent calls.
    #[arg(long)]
    pub(crate) debug: bool,

    /// Auto-approve control-plane prompts.
    #[arg(long)]
    pub(crate) yes: bool,

    /// Auto-approve every approval prompt, including control-plane writes.
    #[arg(long = "always-allow")]
    pub(crate) always_allow: bool,

    /// Run the local HTTP UI (browser) instead of the terminal UI. Defaults to loopback-only.
    #[arg(long = "http", conflicts_with = "tui")]
    pub(crate) http: bool,
    /// Run as an MCP server over stdio (Model Context Protocol, JSON-RPC 2.0): MCP
    /// clients (Claude Code, Codex, IDEs) can call theway's local-execution tools as
    /// standard MCP tools. Mutually exclusive with the other UI modes.
    #[arg(long = "mcp", conflicts_with_all = ["http", "grpc", "tui"])]
    pub(crate) mcp: bool,
    /// Run a local gRPC server instead of the terminal UI. Loopback-only; exposes the same
    /// command/snapshot surface as `--http` (state, events stream, prompt, model, abort,
    /// control-plane resolve) over tonic.
    #[arg(long, conflicts_with = "http")]
    pub(crate) grpc: bool,
    /// Run the terminal UI even when local defaults would open the HTTP UI.
    #[arg(long, conflicts_with = "http")]
    pub(crate) tui: bool,
    /// Host for `--http`/`--grpc`. Must be a loopback address.
    #[arg(long = "http-host", default_value = "127.0.0.1", value_name = "HOST")]
    pub(crate) http_host: String,
    /// Port for `--http`/`--grpc`; use 0 to bind a random free port.
    #[arg(long = "http-port", default_value_t = 0, value_name = "PORT")]
    pub(crate) http_port: u16,
}

#[derive(Subcommand, Debug)]
pub(crate) enum CliCommand {
    /// Export or import replayable `.theway-session` backups.
    Session {
        #[command(subcommand)]
        command: SessionCliCommand,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum SessionCliCommand {
    /// Export a session transcript and automation sidecars to a `.theway-session` archive.
    Export {
        /// Session id to export (full UUIDv7 or unique prefix). Defaults to newest for this cwd.
        #[arg(long, conflicts_with = "current")]
        session: Option<String>,
        /// Export the newest session for this cwd.
        #[arg(long)]
        current: bool,
        /// Destination `.theway-session` file. Defaults to `theway-session-<id>.theway-session` in cwd.
        #[arg(long, value_name = "FILE")]
        output: Option<std::path::PathBuf>,
        /// Do not include dynamic trigger or cron sidecars.
        #[arg(long = "exclude-triggers")]
        exclude_triggers: bool,
    },
    /// Import a `.theway-session` archive as a new local session.
    Import {
        /// `.theway-session` archive to import.
        file: std::path::PathBuf,
        /// Cwd to write into the imported session metadata. Defaults to the current directory.
        #[arg(long, value_name = "PATH")]
        cwd: Option<std::path::PathBuf>,
        /// Activation mode for imported triggers/crons. Defaults to disabled; ask is reserved.
        #[arg(long = "activate-triggers", default_value = "off")]
        activate_triggers: ActivateTriggersArg,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ActivateTriggersArg {
    Off,
    Ask,
    On,
}

impl From<ActivateTriggersArg> for session_archive::ActivateTriggers {
    fn from(value: ActivateTriggersArg) -> Self {
        match value {
            ActivateTriggersArg::Off => Self::Off,
            ActivateTriggersArg::Ask => Self::Ask,
            ActivateTriggersArg::On => Self::On,
        }
    }
}

impl Cli {
    /// Session id to resume, merging both spellings: `--resume-id <id>` wins, then
    /// `--resume <id>`. Bare `--resume` (the picker) yields `None`.
    pub(crate) fn effective_resume_id(&self) -> Option<&str> {
        self.resume_id
            .as_deref()
            .or_else(|| self.resume.as_ref().and_then(|inner| inner.as_deref()))
    }
}

pub(crate) fn print_session_archive_warning() {
    println!(
        "warning: .theway-session archives include transcript and tool history. They do not include separate auth stores, provider credentials, OAuth tokens, or MCP config."
    );
}

pub(crate) fn short_id(id: &str) -> String {
    id.chars().take(16).collect()
}

pub(crate) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub(crate) fn print_dynamic_help_and_exit_if_requested() -> Result<()> {
    if !should_print_dynamic_top_level_help(std::env::args_os().skip(1)) {
        return Ok(());
    }
    let mut command = Cli::command().after_help(commands::cli_model_help_text());
    command.print_help()?;
    println!();
    std::process::exit(0);
}

fn should_print_dynamic_top_level_help<I>(args: I) -> bool
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let subcommands: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|command| command.get_name().to_string())
        .collect();
    let mut has_help = false;
    for arg in args {
        if arg == "--help" || arg == "-h" {
            has_help = true;
            continue;
        }
        if arg
            .to_str()
            .is_some_and(|arg| subcommands.iter().any(|subcommand| subcommand == arg))
        {
            return false;
        }
    }
    has_help
}

pub(crate) async fn list_sessions_cmd(repo: &JsonlSessionRepo) -> Result<()> {
    let entries = session::list_entries(repo).await?;
    if entries.is_empty() {
        println!("(no sessions for this cwd)");
        return Ok(());
    }
    println!("sessions in {}:", repo.root().display());
    for e in entries {
        let preview = e.preview.as_deref().unwrap_or("");
        let badge = e
            .automation
            .badge()
            .map(|b| format!("  [{b}]"))
            .unwrap_or_default();
        println!(
            "  {}  {}{}  {}",
            &e.id[..16.min(e.id.len())],
            e.created_at,
            badge,
            preview
        );
    }
    Ok(())
}

/// List sessions across every cwd-hash bucket under `<base>/sessions/`. For each session we
/// show: short id, the cwd it was created from, created-at timestamp, first user-message
/// preview.
pub(crate) async fn list_all_sessions_cmd() -> Result<()> {
    let root = config::base_dir().join("sessions");
    if !root.exists() {
        println!("(no sessions root: {})", root.display());
        return Ok(());
    }
    let mut buckets = Vec::new();
    let mut rd = tokio::fs::read_dir(&root)
        .await
        .with_context(|| format!("read {}", root.display()))?;
    while let Some(entry) = rd.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            buckets.push(entry.path());
        }
    }
    buckets.sort();

    let mut all = Vec::new();
    for b in &buckets {
        let repo = theway_core::JsonlSessionRepo::new(b);
        // list_entries may return Err if the bucket is empty/malformed; skip those gracefully.
        let entries = session::list_entries(&repo).await.unwrap_or_default();
        for e in entries {
            all.push((b.clone(), e));
        }
    }
    if all.is_empty() {
        println!("(no sessions found under {})", root.display());
        return Ok(());
    }
    // Sort by session id (UUIDv7, time-ordered) so newest is last in output.
    all.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    println!("All sessions ({}):", all.len());
    for (bucket, e) in all {
        let bucket_name = bucket.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let preview = e.preview.as_deref().unwrap_or("");
        let id_short: String = e.id.chars().take(16).collect();
        let badge = e
            .automation
            .badge()
            .map(|b| format!("  [{b}]"))
            .unwrap_or_default();
        println!(
            "  {bucket_name}/{id_short}  {}{badge}  {preview}",
            e.created_at
        );
    }
    Ok(())
}

pub(crate) async fn delete_session_cmd(repo: &JsonlSessionRepo, id: &str) -> Result<()> {
    let path = session::delete_by_id(repo, id).await?;
    println!("deleted {}", path.display());
    Ok(())
}

pub(crate) async fn select_resume_session(
    repo: &JsonlSessionRepo,
    cwd: &std::path::Path,
) -> Result<(theway_core::Session, bool)> {
    let mut entries = session::list_entries(repo).await?;
    if entries.is_empty() {
        anyhow::bail!("no sessions to resume in {}", repo.root().display());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "multiple sessions found in {}; run `theway --list-sessions` and resume one with `theway --resume-id <id>`",
            repo.root().display()
        );
    }

    entries.reverse(); // newest first — index 0 in the picker is the latest session
    let rows: Vec<resume_picker::PickerRow> = entries
        .iter()
        .map(|entry| resume_picker::PickerRow {
            id_short: entry.id.chars().take(16).collect(),
            // RFC3339 with sub-second precision is noise in a menu; minutes are enough.
            created_at: entry.created_at.chars().take(16).collect(),
            badge: entry.automation.badge(),
            preview: entry.preview.clone().unwrap_or_default(),
        })
        .collect();
    let choice = tokio::task::spawn_blocking(move || resume_picker::pick_blocking(&rows))
        .await
        .context("resume picker task")??;
    match choice {
        resume_picker::PickerChoice::Clean => Ok((session::create(repo, cwd).await?, false)),
        resume_picker::PickerChoice::Resume(selected) => {
            Ok((repo.open(&entries[selected].path).await?, true))
        }
        resume_picker::PickerChoice::Cancelled => anyhow::bail!("resume selection cancelled"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_flag_accepts_optional_session_id() {
        use clap::Parser;
        // Bare --resume keeps the picker behavior.
        let bare = Cli::try_parse_from(["theway", "--resume"]).expect("bare --resume parses");
        assert!(bare.resume.is_some());
        assert_eq!(bare.effective_resume_id(), None);

        // --resume <id> behaves like --resume-id <id>.
        let with_id =
            Cli::try_parse_from(["theway", "--resume", "019ea2fd"]).expect("--resume with id");
        assert_eq!(with_id.effective_resume_id(), Some("019ea2fd"));

        // --resume-id still works and wins when both are given.
        let both =
            Cli::try_parse_from(["theway", "--resume", "aaa", "--resume-id", "bbb"]).expect("both");
        assert_eq!(both.effective_resume_id(), Some("bbb"));

        // --resume followed by another flag must not swallow the flag as its value.
        let with_flag =
            Cli::try_parse_from(["theway", "--resume", "--http"]).expect("flag not swallowed");
        assert_eq!(with_flag.effective_resume_id(), None);
        assert!(with_flag.resume.is_some());
        assert!(with_flag.http);

        // Absent entirely.
        let none = Cli::try_parse_from(["theway"]).expect("no flags");
        assert!(none.resume.is_none());
        assert_eq!(none.effective_resume_id(), None);
    }

    #[test]
    fn cli_parses_session_export_import_commands() {
        let cli = Cli::parse_from([
            "theway",
            "session",
            "export",
            "--session",
            "018f",
            "--output",
            "backup.theway-session",
            "--exclude-triggers",
        ]);
        match cli.command {
            Some(CliCommand::Session {
                command:
                    SessionCliCommand::Export {
                        session,
                        output,
                        exclude_triggers,
                        ..
                    },
            }) => {
                assert_eq!(session.as_deref(), Some("018f"));
                assert_eq!(
                    output.unwrap(),
                    std::path::PathBuf::from("backup.theway-session")
                );
                assert!(exclude_triggers);
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::parse_from([
            "theway",
            "session",
            "import",
            "backup.theway-session",
            "--activate-triggers",
            "off",
        ]);
        assert!(matches!(
            cli.command,
            Some(CliCommand::Session {
                command: SessionCliCommand::Import {
                    activate_triggers: ActivateTriggersArg::Off,
                    ..
                }
            })
        ));
    }

    #[tokio::test]
    async fn cli_session_import_ask_imports_disabled_first() {
        // `ask` must never reach the archive layer as Ask: the import itself runs with
        // Off, and the interactive offer happens afterwards (TTY only). With a missing
        // archive the failure is the archive read — not an "ask unsupported" error.
        let temp = tempfile::tempdir().unwrap();
        let repo = JsonlSessionRepo::new(temp.path().join("sessions"));
        let command = SessionCliCommand::Import {
            file: std::path::PathBuf::from("missing.theway-session"),
            cwd: None,
            activate_triggers: ActivateTriggersArg::Ask,
        };
        let err = crate::run_session_cli_command(&command, &repo, temp.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("not implemented"),
            "ask is implemented now: {err}"
        );
        assert!(err.contains("missing.theway-session"), "{err}");
    }
}
