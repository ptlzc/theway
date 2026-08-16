//! CLI argument types and standalone session commands for the `theway` binary.
//!
//! Split out of `main.rs` (which keeps the `fn main` entry point and the
//! `run_cli_command` / `run_session_cli_command` dispatch). Mechanical module
//! extraction — behavior is unchanged.
//!
//! Issue #64: the read/only session surfaces (`--list-sessions`,
//! `--delete-session`) prefer the running daemon's session RPCs over opening
//! the cwd-scoped SQLite repo themselves — the daemon owns that repo and
//! holds the libsql lock on its live session, so a concurrent TUI read/write
//! risks lock contention and double bookkeeping. The local repo path remains
//! as a clearly-labeled offline fallback (no daemon answering for this cwd);
//! these commands never spawn a daemon.

use std::io::IsTerminal as _;

use crate::resume_picker;
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use theway_storage::session;
use theway_storage::session_archive;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::client::{GrpcClient, discover};
use theway_transport::commands;
use theway_transport::config;
use theway_transport::wire::SessionSummary;

#[derive(Parser, Debug)]
#[command(
    name = "theway",
    version,
    about = "theway client — connects to the thewayd daemon (spawns one when absent); the daemon owns the agent runtime"
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

    /// List sessions for this cwd and exit. Asks the running daemon first;
    /// falls back to the local session repo when no daemon is running.
    #[arg(long)]
    pub(crate) list_sessions: bool,
    /// List sessions across every cwd we know about (~/.theway/sessions/*) and exit.
    #[arg(long)]
    pub(crate) list_all_sessions: bool,
    /// Delete a session by id and exit. Asks the running daemon first (which
    /// refuses while the session still has running graphs); falls back to
    /// the local session repo when no daemon is running.
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

    /// Explicitly run the terminal UI (the default on a TTY).
    #[arg(long)]
    pub(crate) tui: bool,
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

/// Probe for a running daemon for `cwd` and connect to it; `None` when
/// offline. Discovery only — this never spawns a daemon: standalone session
/// commands (`--list-sessions`, `--delete-session`) must not start a
/// background process just to run (issue #64).
async fn connect_running_daemon(cwd: &std::path::Path) -> Option<GrpcClient> {
    let addr = discover(std::time::Duration::from_millis(800), cwd)
        .await
        .ok()
        .flatten()?;
    GrpcClient::connect(&addr).await.ok()
}

/// Offline-fallback notice for the standalone session commands (issue #64):
/// no daemon answered for this cwd, so the command reads/writes the local
/// session repo directly.
fn print_offline_fallback_notice() {
    println!(
        "note: no running daemon for this cwd — falling back to the local session repo (offline)"
    );
}

/// `--list-sessions` (issue #64): prefer the running daemon's `list_sessions`
/// RPC — the daemon owns the cwd-scoped repo and holds the libsql lock on its
/// live session, so reading locally while it runs risks lock contention.
/// Only when no daemon answers do we open the local repo (offline fallback,
/// clearly labeled). Never spawns a daemon.
pub(crate) async fn list_sessions_cmd(cwd: &std::path::Path) -> Result<()> {
    if let Some(mut client) = connect_running_daemon(cwd).await {
        return list_sessions_online(&mut client).await;
    }
    print_offline_fallback_notice();
    let repo = session::open_repo(cwd).await;
    list_sessions_offline(&repo).await
}

/// Online `--list-sessions`: render the daemon's session table (flat, oldest
/// → newest) with the live `current` / `busy` / graph marks only the daemon
/// can report.
async fn list_sessions_online(client: &mut GrpcClient) -> Result<()> {
    let (sessions, current_id) = client
        .list_sessions()
        .await
        .context("list sessions from the running daemon")?;
    if sessions.is_empty() {
        println!("(no sessions for this cwd)");
        return Ok(());
    }
    println!("sessions for this cwd (live, from the running daemon):");
    for summary in &sessions {
        println!(
            "{}",
            online_session_row(summary, summary.session_id == current_id)
        );
    }
    Ok(())
}

/// One online listing row: short id, name (when set), created-at, live marks
/// (`current` / `busy` / graph counts in a badge), then the first user
/// message preview — mirroring the offline row shape where the wire model
/// allows (no fork lineage: the wire summary carries no parent id).
fn online_session_row(summary: &SessionSummary, is_current: bool) -> String {
    let mut marks = Vec::new();
    if is_current {
        marks.push("current".to_string());
    }
    if summary.busy {
        marks.push("busy".to_string());
    }
    if summary.graph_count > 0 {
        marks.push(if summary.active_graph_count > 0 {
            format!(
                "graphs {} ({} active)",
                summary.graph_count, summary.active_graph_count
            )
        } else {
            format!("graphs {}", summary.graph_count)
        });
    }
    let badge = if marks.is_empty() {
        String::new()
    } else {
        format!("  [{}]", marks.join(", "))
    };
    let name = if summary.name.is_empty() {
        String::new()
    } else {
        format!("  {}", summary.name)
    };
    let preview = summary.preview.as_deref().unwrap_or("(empty)");
    format!(
        "  {}{}  {}{badge}  {}",
        short_id(&summary.session_id),
        name,
        summary.created_at,
        preview
    )
}

/// Offline `--list-sessions`: read the cwd-scoped repo directly (the pre-#64
/// behavior; tree view with forks nested under their parent).
async fn list_sessions_offline(repo: &SqliteSessionRepo) -> Result<()> {
    let entries = session::list_entries(repo).await?;
    if entries.is_empty() {
        println!("(no sessions for this cwd)");
        return Ok(());
    }
    println!(
        "sessions in {} (tree — forks nested under their parent):",
        repo.root().display()
    );
    for row in session::flatten_session_tree(&entries) {
        let preview = row.preview.as_deref().unwrap_or("(empty)");
        let badge = row
            .automation
            .badge()
            .map(|b| format!("  [{b}]"))
            .unwrap_or_default();
        println!(
            "  {}{}  {}{badge}  {}",
            row.prefix,
            &row.id[..16.min(row.id.len())],
            row.created_at,
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
        let repo = SqliteSessionRepo::new(b);
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

/// `--delete-session` (issue #64): prefer the daemon's `delete_session` RPC —
/// only the daemon can enforce delete protection against live DAG runs, and
/// deleting locally while it runs races its repo lock. Falls back to the
/// local repo only when no daemon answers; never spawns one.
pub(crate) async fn delete_session_cmd(cwd: &std::path::Path, id: &str) -> Result<()> {
    if let Some(mut client) = connect_running_daemon(cwd).await {
        return delete_session_online(&mut client, id).await;
    }
    print_offline_fallback_notice();
    let repo = session::open_repo(cwd).await;
    delete_session_offline(&repo, id).await
}

/// Online delete via the daemon RPC. Delete protection (running DAG runs
/// still attached to the session) reports the refusal reason and exits
/// non-zero; `Ok` means the session is gone.
async fn delete_session_online(client: &mut GrpcClient, id: &str) -> Result<()> {
    match client.delete_session(id).await {
        // Contract-level refusal: the response itself names the running runs.
        Ok(running) if !running.is_empty() => {
            anyhow::bail!(
                "delete refused: session {id} still has running graphs: {} — cancel them and retry",
                running.join(", ")
            )
        }
        Ok(_) => {
            println!("deleted session {id}");
            Ok(())
        }
        // The gRPC surface maps the same refusal onto `failed_precondition`;
        // surface its reason instead of the raw RPC error. Everything else
        // (session not found, transport failure) propagates unchanged.
        Err(e) => match delete_refusal_reason(&e) {
            Some(reason) => anyhow::bail!("delete refused: {reason}"),
            None => Err(e),
        },
    }
}

/// Extract the daemon's delete-protection reason from an RPC error: the gRPC
/// `delete_session` handler refuses with `failed_precondition` carrying
/// "session <id> still has running graphs: <run ids>; ...". `None` when the
/// failure is unrelated (session not found, transport error, ...).
fn delete_refusal_reason(err: &anyhow::Error) -> Option<String> {
    let text = err.to_string();
    let marker = "still has running graphs";
    let marker_at = text.find(marker)?;
    // Widen left to the sentence start ("session <id> ..."), right to the
    // ";" before the daemon's cancel hint.
    let start = text[..marker_at].rfind("session ").unwrap_or(marker_at);
    let end = text[marker_at..]
        .find(';')
        .map(|i| marker_at + i)
        .unwrap_or(text.len());
    Some(text[start..end].to_string())
}

/// Offline delete: the pre-#64 local-repo path. With no daemon running there
/// are no live DAG runs, so delete protection cannot apply here.
async fn delete_session_offline(repo: &SqliteSessionRepo, id: &str) -> Result<()> {
    let path = session::delete_by_id(repo, id).await?;
    println!("deleted {}", path.display());
    Ok(())
}

pub(crate) async fn select_resume_session(
    repo: &SqliteSessionRepo,
    cwd: &std::path::Path,
) -> Result<(theway_core::Session, bool)> {
    let entries = session::list_entries(repo).await?;
    if entries.is_empty() {
        anyhow::bail!("no sessions to resume in {}", repo.root().display());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "multiple sessions found in {}; run `theway --list-sessions` and resume one with `theway --resume-id <id>`",
            repo.root().display()
        );
    }

    // Chronological tree order (oldest → newest, forks nested under parents).
    let tree = session::flatten_session_tree(&entries);
    let rows: Vec<resume_picker::PickerRow> = tree
        .iter()
        .map(|row| resume_picker::PickerRow {
            id_short: row.id.chars().take(16).collect(),
            // RFC3339 with sub-second precision is noise in a menu; minutes are enough.
            created_at: row.created_at.chars().take(16).collect(),
            badge: row.automation.badge(),
            preview: row.preview.clone().unwrap_or_default(),
            prefix: row.prefix.clone(),
        })
        .collect();
    let choice = tokio::task::spawn_blocking(move || resume_picker::pick_blocking(&rows))
        .await
        .context("resume picker task")??;
    match choice {
        resume_picker::PickerChoice::Clean => Ok((session::create(repo, cwd).await?, false)),
        resume_picker::PickerChoice::Resume(selected) => {
            Ok((repo.open(&tree[selected].path).await?, true))
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
            Cli::try_parse_from(["theway", "--resume", "--tui"]).expect("flag not swallowed");
        assert_eq!(with_flag.effective_resume_id(), None);
        assert!(with_flag.resume.is_some());
        assert!(with_flag.tui);

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
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
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

    // ── issue #64: online/offline session command branches ───────────────

    fn summary(id: &str) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            name: String::new(),
            cwd: "/tmp/theway".to_string(),
            model: "provider:model".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_activity_at: 0,
            graph_count: 0,
            active_graph_count: 0,
            busy: false,
            preview: Some("hello".to_string()),
        }
    }

    #[test]
    fn online_session_row_renders_marks_and_preview() {
        // Plain row: short id (first 16 chars, dashes included — same as the
        // offline listing), created-at, preview.
        let plain = online_session_row(&summary("019ea2fd-0000-7000-8000-000000000000"), false);
        assert_eq!(plain, "  019ea2fd-0000-70  2026-01-01T00:00:00Z  hello");

        // Live marks join in one badge; `current` + `busy` + graph counts.
        let mut current = summary("019ea2fd-0000-7000-8000-000000000000");
        current.busy = true;
        current.graph_count = 2;
        current.active_graph_count = 1;
        let row = online_session_row(&current, true);
        assert!(
            row.contains("[current, busy, graphs 2 (1 active)]"),
            "{row}"
        );

        // Graphs without an active run render the plain count.
        let mut idle = summary("019ea2fd-0000-7000-8000-000000000000");
        idle.graph_count = 3;
        let row = online_session_row(&idle, false);
        assert!(row.contains("[graphs 3]"), "{row}");

        // A set name follows the id; a missing preview renders "(empty)".
        let mut named = summary("019ea2fd-0000-7000-8000-000000000000");
        named.name = "refactor".to_string();
        named.preview = None;
        let row = online_session_row(&named, false);
        assert!(row.starts_with("  019ea2fd-0000-70  refactor  "), "{row}");
        assert!(row.ends_with("(empty)"), "{row}");
    }

    #[test]
    fn delete_refusal_reason_parses_failed_precondition_message() {
        // The gRPC delete handler wraps the daemon's refusal as a tonic
        // `failed_precondition`; the client surfaces it inside its anyhow
        // context. The parser must lift out the human sentence only.
        let err = anyhow::anyhow!(
            "delete_session: code: 'failed precondition', message: \"session 019ea2fd still has running graphs: run-1, run-2; cancel them (GraphCancel) before deleting\""
        );
        assert_eq!(
            delete_refusal_reason(&err).as_deref(),
            Some("session 019ea2fd still has running graphs: run-1, run-2")
        );

        // Unrelated failures (not found, transport) are not refusals.
        let not_found = anyhow::anyhow!(
            "delete_session: code: 'not found', message: \"no session matches id nope\""
        );
        assert!(delete_refusal_reason(&not_found).is_none());
    }

    #[tokio::test]
    async fn list_sessions_online_round_trips_through_daemon_rpc() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1", "sess-2"])
                .await;
        // Smoke: the online branch drives the real RPC surface without error.
        list_sessions_online(&mut client).await.unwrap();

        // An empty daemon renders the same "(no sessions …)" line as offline.
        let (mut empty, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&[]).await;
        list_sessions_online(&mut empty).await.unwrap();
    }

    #[tokio::test]
    async fn delete_session_online_deletes_via_daemon_rpc() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1", "sess-2"])
                .await;
        delete_session_online(&mut client, "sess-2").await.unwrap();
        let (sessions, _) = client.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn delete_session_online_refuses_while_graphs_run() {
        let (mut client, _rx, ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1"]).await;
        ops.set_running("sess-1", &["run-1", "run-2"]);

        let err = delete_session_online(&mut client, "sess-1")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("delete refused"), "{err}");
        assert!(err.contains("still has running graphs"), "{err}");
        assert!(err.contains("run-1") && err.contains("run-2"), "{err}");

        // The session survives the refused delete.
        let (sessions, _) = client.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
    }

    #[tokio::test]
    async fn delete_session_online_unknown_id_propagates_not_found() {
        let (mut client, _rx, _ops) =
            crate::startup::test_daemon::test_daemon_client_with_sessions(&["sess-1"]).await;
        let err = delete_session_online(&mut client, "nope")
            .await
            .unwrap_err()
            .to_string();
        // Not-found is NOT a delete-protection refusal — the raw RPC error
        // propagates (no "delete refused" mapping).
        assert!(!err.contains("delete refused"), "{err}");
        assert!(err.contains("no session matches id"), "{err}");
    }

    async fn session_id_of(session: &theway_core::Session) -> String {
        session
            .storage()
            .get_metadata_json()
            .await
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn list_sessions_offline_reads_local_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;
        drop(session);

        // The offline branch is the pre-#64 local-repo path; it must list the
        // freshly created session without a daemon in the loop.
        list_sessions_offline(&repo).await.unwrap();
        let entries = session::list_entries(&repo).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);
    }

    #[tokio::test]
    async fn delete_session_offline_removes_from_local_repo() {
        let temp = tempfile::tempdir().unwrap();
        let repo = SqliteSessionRepo::new(temp.path().join("sessions"));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;
        drop(session);

        delete_session_offline(&repo, &id).await.unwrap();
        assert!(session::list_entries(&repo).await.unwrap().is_empty());

        // Deleting a missing id errors just like the pre-#64 behavior.
        let err = delete_session_offline(&repo, "nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches id nope"), "{err}");
    }
}
