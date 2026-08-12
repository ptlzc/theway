//! theway — coding agent CLI binary (`[[bin]]` of the `theway` crate). Thin assembly layer
//! on top of the `theway` SDK library; all runtime logic lives in the library so external
//! projects can embed it in-process. `--http` / `--grpc` dispatch into the `transport::http`
//! / `transport::grpc` drivers (crate `server` feature). The bin target requires features
//! `tui` + `server`, so both are compile-time constants here.
//!
//! Modeled on `packages/coding-agent/` (the TS implementation) in spirit: same tools
//! (`read`/`write`/`edit`/`bash`/`ls`/`grep`/`find` + `memory`), same `--resume` semantics
//! scoped by cwd hash, same "interactive TUI" mode, dual-root skills loader (project ↻ user).
//! Trimmed scope: no extensions, no themes, no print/rpc/json modes.
//!
//! The bin entry is split into submodule directories: [`cli`] (CLI argument types +
//! standalone session commands), [`startup`] (`run_repl` startup assembly),
//! [`session_factory`] (per-session harness rebuild), and [`ui_mode`] (UI mode
//! resolution + small CLI-level helpers). This file keeps `fn main` and the CLI
//! command dispatch.

mod cli;
mod session_factory;
mod startup;
mod ui_mode;

use std::io::IsTerminal as _;

use anyhow::{Context, Result};
use clap::Parser;
use theway::{session, session_archive};
use theway_core::JsonlSessionRepo;

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
    let repo = session::open_repo(&cwd).await;

    if let Some(command) = &cli.command {
        return run_cli_command(command, &repo, &cwd).await;
    }

    if cli.list_sessions {
        return list_sessions_cmd(&repo).await;
    }
    if cli.list_all_sessions {
        return list_all_sessions_cmd().await;
    }
    if let Some(id) = &cli.delete_session {
        return delete_session_cmd(&repo, id).await;
    }

    run_repl(cli, cwd, repo).await
}

async fn run_cli_command(
    command: &CliCommand,
    repo: &JsonlSessionRepo,
    cwd: &std::path::Path,
) -> Result<()> {
    match command {
        CliCommand::Session { command } => run_session_cli_command(command, repo, cwd).await,
    }
}

async fn run_session_cli_command(
    command: &SessionCliCommand,
    repo: &JsonlSessionRepo,
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
                session_archive::export_session(&session_path, &output_path, *exclude_triggers)
                    .await?;
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
