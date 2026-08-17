//! theway — coding agent TUI client binary (bin `theway` of the `theway-tui` crate).
//!
//! Pure client of the `thewayd` daemon kernel: on startup it reuses a running daemon
//! (per-cwd discovery file or default port) or spawns `thewayd` in the current
//! directory and waits for readiness, then runs the ratatui REPL against the transport
//! client. The agent runtime (harness, session, tools, triggers) lives in the daemon;
//! this crate links `theway-transport` / `theway-core` / `theway-storage` and never the
//! daemon kernel.
//!
//! Offline session maintenance is the exception: `session export|import` and the
//! standalone session queries (`--list-sessions`, `--list-all-sessions`,
//! `--delete-session`) open the local SQLite session repo directly, without the daemon.
//!
//! The bin entry is split into submodule directories: [`cli`] (CLI argument types +
//! standalone session commands), [`startup`] (`run_repl`: daemon discovery/spawn +
//! connect), and [`ui`] (the ratatui REPL); feed rendering, local commands, model/resume
//! pickers, and clipboard image support live in the sibling modules. This file keeps
//! `fn main` and the CLI command dispatch.

mod cli;
mod clipboard_image;
mod config_payload;
mod feed_cache;
mod feed_render;
mod local_commands;
mod local_tool_ops;
mod model_picker;
mod resume_picker;
mod startup;
pub mod ui;

use std::io::IsTerminal as _;

use anyhow::{Context, Result};
use clap::Parser;
use theway_storage::session;
use theway_storage::session_archive;
use theway_storage::sqlite_repo::SqliteSessionRepo;

use cli::{
    ActivateTriggersArg, Cli, CliCommand, SessionCliCommand, delete_session_cmd,
    list_all_sessions_cmd, list_sessions_cmd, print_dynamic_help_and_exit_if_requested,
    print_session_archive_warning, short_id, yes_no,
};
use startup::run_repl;

// Test/feed helper kept at crate-root visibility (it was `pub` on the old monolithic
// `main.rs`); re-exported so `crate::user_message` resolves exactly as before.
pub use startup::user_message;

#[tokio::main]
async fn main() -> Result<()> {
    print_dynamic_help_and_exit_if_requested()?;
    let cli = Cli::parse();
    let cwd = std::env::current_dir().context("getting cwd")?;

    // Session export/import keep their existing offline, repo-direct path.
    if let Some(command) = &cli.command {
        let repo = session::open_repo(&cwd).await;
        return run_cli_command(command, &repo, &cwd).await;
    }

    // Standalone session queries (issue #64): try the running daemon's RPC
    // first and only open the local repo as an offline fallback inside the
    // command — opening the repo here would race the daemon's libsql lock
    // before we even know whether a daemon is up.
    if cli.list_sessions {
        return list_sessions_cmd(&cwd).await;
    }
    if cli.list_all_sessions {
        return list_all_sessions_cmd().await;
    }
    if let Some(id) = &cli.delete_session {
        return delete_session_cmd(&cwd, id).await;
    }

    // Interactive REPL + resume picker: the local repo is still needed (the
    // daemon resolves its own session from the launch args; the TUI needs the
    // repo for the picker and its local surfaces).
    let repo = session::open_repo(&cwd).await;
    run_repl(cli, cwd, repo).await
}

async fn run_cli_command(
    command: &CliCommand,
    repo: &SqliteSessionRepo,
    cwd: &std::path::Path,
) -> Result<()> {
    match command {
        CliCommand::Session { command } => run_session_cli_command(command, repo, cwd).await,
    }
}

async fn run_session_cli_command(
    command: &SessionCliCommand,
    repo: &SqliteSessionRepo,
    cwd: &std::path::Path,
) -> Result<()> {
    match command {
        SessionCliCommand::Export {
            session,
            current: _,
            output,
            exclude_triggers,
        } => {
            let session_path = if let Some(id) = session {
                session::find_path_by_id(repo, id)
                    .await?
                    .with_context(|| format!("no session matches id {id}"))?
            } else {
                session::newest_path(repo).await?.with_context(|| {
                    format!("no sessions to export in {}", repo.root().display())
                })?
            };
            let session = repo.open(&session_path).await?;
            let metadata = session.storage().get_metadata_json().await?;
            let session_id = metadata
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| {
                    session_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("session")
                });
            let output_path = output
                .clone()
                .unwrap_or_else(|| session_archive::default_export_path(cwd, session_id));
            let output_path = if output_path.is_absolute() {
                output_path
            } else {
                cwd.join(output_path)
            };
            print_session_archive_warning();
            let summary =
                session_archive::export_session(&session, &output_path, *exclude_triggers).await?;
            println!(
                "exported session archive: {}",
                summary.output_path.display()
            );
            println!(
                "session {} entries={} triggers={} cron={}",
                short_id(&summary.session_id),
                summary.entry_count,
                yes_no(summary.has_triggers),
                yes_no(summary.has_cron)
            );
            Ok(())
        }
        SessionCliCommand::Import {
            file,
            cwd: import_cwd,
            activate_triggers,
        } => {
            let archive_path = if file.is_absolute() {
                file.clone()
            } else {
                cwd.join(file)
            };
            let target_cwd = import_cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
            print_session_archive_warning();
            // `ask` imports disabled first, then offers to restore the source enablement
            // interactively — never pass Ask down to the archive layer.
            let effective = match activate_triggers {
                ActivateTriggersArg::Ask => session_archive::ActivateTriggers::Off,
                other => (*other).into(),
            };
            let summary =
                session_archive::import_session(repo, &archive_path, &target_cwd, effective)
                    .await?;
            println!("imported session: {}", short_id(&summary.session_id));
            println!("path: {}", summary.session_path.display());
            println!(
                "entries={} triggers={} cron={} automation={}",
                summary.entry_count,
                summary.triggers_imported,
                summary.cron_imported,
                if summary.automation_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if *activate_triggers == ActivateTriggersArg::Ask
                && (!summary.originally_enabled_triggers.is_empty()
                    || !summary.originally_enabled_cron.is_empty())
            {
                if std::io::stdin().is_terminal() {
                    print!(
                        "archive had {} trigger(s) and {} cron job(s) enabled — activate them now? [y/N] ",
                        summary.originally_enabled_triggers.len(),
                        summary.originally_enabled_cron.len()
                    );
                    use std::io::Write as _;
                    std::io::stdout().flush().ok();
                    let mut answer = String::new();
                    std::io::stdin().read_line(&mut answer).ok();
                    if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
                        let (triggers, cron) = session_archive::activate_imported(
                            &summary.session_path,
                            &summary.originally_enabled_triggers,
                            &summary.originally_enabled_cron,
                        )?;
                        println!("activated: {triggers} trigger(s), {cron} cron job(s) re-enabled");
                    } else {
                        println!("automation stays disabled");
                    }
                } else {
                    println!(
                        "automation left disabled (no TTY to confirm); re-import with --activate-triggers=on to enable"
                    );
                }
            }
            println!("resume with: theway --resume-id {}", summary.session_id);
            Ok(())
        }
    }
}
