//! Session lifecycle commands: `/save`, `/undo`, `/name`, `/session`, `/share`.
//! (`/sessions`, `/login`, `/logout` moved to the SDK local command set —
//! sdk-split-local-sandbox, node 5-commands-layer.)

use super::*;

use theway_sdk::commands::CommandCtx;

pub struct SaveCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SaveCommand {
    fn name(&self) -> &'static str {
        "save"
    }
    fn description(&self) -> &'static str {
        "export session transcript to Markdown"
    }
    fn usage(&self) -> &'static str {
        "[path]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let dest = if let Some(path) = argv.first() {
            std::path::PathBuf::from(path)
        } else {
            crate::export::default_export_path(ctx.session_id)
        };
        // If the path is relative, resolve against cwd so /save foo.md lands where the user
        // expects (and not in some random working dir).
        let dest = if dest.is_absolute() {
            dest
        } else {
            ctx.cwd.join(dest)
        };
        match crate::export::save(ctx.harness.session(), &dest).await {
            Ok(p) => {
                cprintln!("saved transcript: {}", p.display());
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("save failed: {e}")),
        }
    }
}

pub struct UndoCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for UndoCommand {
    fn name(&self) -> &'static str {
        "undo"
    }
    fn description(&self) -> &'static str {
        "remove the most recent user+assistant turn from the active branch"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let session = ctx.harness.session();
        let path = match session.branch(None).await {
            Ok(p) => p,
            Err(e) => return CommandOutcome::Error(format!("read branch: {e}")),
        };
        // Walk backwards for the most recent Message that's a User. That message is the
        // start of the turn we want to drop.
        let mut target_parent: Option<String> = None;
        let mut found = false;
        for entry in path.iter().rev() {
            if let theway_core::SessionTreeEntry::Message {
                message: theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(_)),
                parent_id,
                ..
            } = entry
            {
                target_parent = parent_id.clone();
                found = true;
                break;
            }
        }
        if !found {
            return CommandOutcome::Error("no user message to undo".into());
        }
        match ctx.harness.move_to(target_parent.as_deref(), None).await {
            Ok(_) => {
                cprintln!("undid last turn");
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("undo failed: {e}")),
        }
    }
}

pub struct NameCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for NameCommand {
    fn name(&self) -> &'static str {
        "name"
    }
    fn description(&self) -> &'static str {
        "show or set the current session's name"
    }
    fn usage(&self) -> &'static str {
        "[slug]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let session = ctx.harness.session();
        if argv.is_empty() {
            match session.session_name().await {
                Ok(Some(n)) => cprintln!("session name: {n}"),
                Ok(None) => cprintln!("(unnamed session)"),
                Err(e) => return CommandOutcome::Error(format!("read name: {e}")),
            }
            return CommandOutcome::Handled;
        }
        let name = argv.join(" ");
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return CommandOutcome::Error("empty name".into());
        }
        match session.append_session_name(trimmed.to_string()).await {
            Ok(_) => {
                cprintln!("session name set to: {trimmed}");
                CommandOutcome::Handled
            }
            Err(e) => CommandOutcome::Error(format!("set name failed: {e}")),
        }
    }
}

pub struct SessionCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SessionCommand {
    fn name(&self) -> &'static str {
        "session"
    }
    fn description(&self) -> &'static str {
        "export/import replayable .theway-session backups"
    }
    fn usage(&self) -> &'static str {
        "export [path] [--exclude-triggers] | import <path>"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        match argv.first().map(String::as_str) {
            Some("export") => session_export_command(&argv[1..], ctx).await,
            Some("import") => session_import_command(&argv[1..], ctx).await,
            Some(other) => CommandOutcome::Error(format!(
                "unknown /session subcommand: {other}; use /session export [path] or /session import <path>"
            )),
            None => CommandOutcome::Error(
                "usage: /session export [path] [--exclude-triggers] | /session import <path>"
                    .into(),
            ),
        }
    }
}

async fn session_export_command(
    argv: &[String],
    ctx: &CommandCtx<'_, DaemonCtx>,
) -> CommandOutcome {
    let mut exclude_triggers = false;
    let mut path_arg: Option<&str> = None;
    for arg in argv {
        if arg == "--exclude-triggers" {
            exclude_triggers = true;
        } else if path_arg.is_none() {
            path_arg = Some(arg);
        } else {
            return CommandOutcome::Error(
                "usage: /session export [path] [--exclude-triggers]".into(),
            );
        }
    }

    let output_path = match path_arg {
        Some(path) => std::path::PathBuf::from(path),
        None => crate::session_archive::default_export_path(ctx.cwd, ctx.session_id),
    };
    let output_path = if output_path.is_absolute() {
        output_path
    } else {
        ctx.cwd.join(output_path)
    };

    emit_session_archive_warning();
    match crate::session_archive::export_session(
        ctx.harness.session(),
        &output_path,
        exclude_triggers,
    )
    .await
    {
        Ok(summary) => {
            cprintln!(
                "exported session archive: {}",
                summary.output_path.display()
            );
            cprintln!(
                "session {} entries={} triggers={} cron={}",
                short_id(&summary.session_id),
                summary.entry_count,
                yes_no(summary.has_triggers),
                yes_no(summary.has_cron)
            );
            CommandOutcome::Handled
        }
        Err(err) => CommandOutcome::Error(format!("session export failed: {err}")),
    }
}

async fn session_import_command(
    argv: &[String],
    ctx: &CommandCtx<'_, DaemonCtx>,
) -> CommandOutcome {
    if argv.len() != 1 {
        return CommandOutcome::Error("usage: /session import <path>".into());
    }
    let archive_path = std::path::PathBuf::from(&argv[0]);
    let archive_path = if archive_path.is_absolute() {
        archive_path
    } else {
        ctx.cwd.join(archive_path)
    };
    let repo = theway_sdk::session::open_repo(ctx.cwd).await;

    emit_session_archive_warning();
    match crate::session_archive::import_session(
        &repo,
        &archive_path,
        ctx.cwd,
        crate::session_archive::ActivateTriggers::Off,
    )
    .await
    {
        Ok(summary) => {
            cprintln!("imported session: {}", short_id(&summary.session_id));
            cprintln!("path: {}", summary.session_path.display());
            cprintln!(
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
            cprintln!("resume with: theway --resume-id {}", summary.session_id);
            if !summary.originally_enabled_triggers.is_empty()
                || !summary.originally_enabled_cron.is_empty()
            {
                return CommandOutcome::SessionImportActivation {
                    session_path: summary.session_path,
                    trigger_ids: summary.originally_enabled_triggers,
                    cron_ids: summary.originally_enabled_cron,
                };
            }
            CommandOutcome::Handled
        }
        Err(err) => CommandOutcome::Error(format!("session import failed: {err}")),
    }
}

fn emit_session_archive_warning() {
    cprintln!(
        "warning: .theway-session archives include transcript and tool history. They do not include separate auth stores, provider credentials, OAuth tokens, or MCP config."
    );
}

fn short_id(id: &str) -> String {
    id.chars().take(16).collect()
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub struct ShareCommand;

/// The `gh` binary to use for `/share`. Defaults to `gh` on PATH; `THEWAY_GH_BIN`
/// overrides it (gh installed outside PATH, or a test shim).
fn gh_bin() -> String {
    std::env::var("THEWAY_GH_BIN").unwrap_or_else(|_| "gh".to_string())
}

#[async_trait]
impl SlashCommand<DaemonCtx> for ShareCommand {
    fn name(&self) -> &'static str {
        "share"
    }
    fn description(&self) -> &'static str {
        "upload transcript as a private Gist via gh (requires `gh` on PATH)"
    }
    fn usage(&self) -> &'static str {
        "[--public]"
    }
    async fn run(&self, argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let public = argv.iter().any(|a| a == "--public");

        // Render and write to a temp file so gh gist create can ingest it.
        let dir = std::env::temp_dir().join(format!("theway-share-{}", ctx.session_id));
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            return CommandOutcome::Error(format!("share tmp dir: {e}"));
        }
        let file = dir.join("transcript.md");
        if let Err(e) = crate::export::save(ctx.harness.session(), &file).await {
            return CommandOutcome::Error(format!("save transcript: {e}"));
        }

        let mut cmd = tokio::process::Command::new(gh_bin());
        cmd.arg("gist").arg("create");
        if public {
            cmd.arg("--public");
        }
        cmd.arg("--desc")
            .arg(format!("theway session {}", ctx.session_id))
            .arg(file.as_os_str());

        let output = match cmd.output().await {
            Ok(o) => o,
            Err(e) => {
                return CommandOutcome::Error(format!(
                    "gh gist create failed to spawn: {e}. Is gh on PATH?"
                ));
            }
        };
        if !output.status.success() {
            return CommandOutcome::Error(format!(
                "gh gist create exited {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        cprintln!("shared: {url}");
        CommandOutcome::Handled
    }
}
