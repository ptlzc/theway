//! Auth-store slash commands (daemon-kernel-layers: `/login`, `/logout`,
//! `/sessions` moved from the SDK into the daemon's runtime command set).
//!
//! `/login` returns [`CommandOutcome::LoginSecret`] so the client prompts for
//! the secret without echoing it and writes the shared auth store; the daemon
//! picks the key up on its next turn. `/logout` and `/sessions` run headless.

use async_trait::async_trait;
use theway_transport::auth::AuthStore;
use theway_transport::commands::{CommandCtx, CommandOutcome, SlashCommand};

use super::DaemonCtx;

pub struct LoginCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for LoginCommand {
    fn name(&self) -> &'static str {
        "login"
    }
    fn description(&self) -> &'static str {
        "store an API key for a provider in ~/.theway/auth.json"
    }
    fn usage(&self) -> &'static str {
        "<provider>"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        if argv.len() != 1 {
            return CommandOutcome::Error(
                "usage: /login <provider>  (theway will prompt for the API key without echoing it)"
                    .into(),
            );
        }
        CommandOutcome::LoginSecret {
            provider: argv[0].clone(),
            storage_key: None,
            recovery_command: None,
        }
    }
}

pub struct LogoutCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for LogoutCommand {
    fn name(&self) -> &'static str {
        "logout"
    }
    fn description(&self) -> &'static str {
        "remove a stored credential from ~/.theway/auth.json"
    }
    fn usage(&self) -> &'static str {
        "<provider>"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        if argv.is_empty() {
            return CommandOutcome::Error("usage: /logout <provider>".into());
        }
        let provider = &argv[0];
        let mut store = match AuthStore::load() {
            Ok(s) => s,
            Err(e) => return CommandOutcome::Error(format!("load auth store: {e}")),
        };
        match store.remove(provider) {
            Some(_) => match store.save() {
                Ok(()) => {
                    cprintln!("removed credential for `{provider}`");
                    CommandOutcome::Handled
                }
                Err(e) => CommandOutcome::Error(format!("save auth store: {e}")),
            },
            None => {
                cprintln!("no credential stored for `{provider}`");
                CommandOutcome::Handled
            }
        }
    }
}

pub struct SessionsCommand;

#[async_trait]
impl SlashCommand<DaemonCtx> for SessionsCommand {
    fn name(&self) -> &'static str {
        "sessions"
    }
    fn description(&self) -> &'static str {
        "list sessions for this cwd"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_, DaemonCtx>) -> CommandOutcome {
        let repo = theway_storage::session::open_repo(ctx.cwd).await;
        let entries = match theway_storage::session::list_entries(&repo).await {
            Ok(e) => e,
            Err(e) => return CommandOutcome::Error(format!("list sessions: {e}")),
        };
        if entries.is_empty() {
            cprintln!("(no sessions for this cwd)");
            return CommandOutcome::Handled;
        }
        cprintln!("Sessions (tree — forks nested under their parent):");
        for row in theway_storage::session::flatten_session_tree(&entries) {
            let preview = row.preview.as_deref().unwrap_or("(empty)");
            let badge = row
                .automation
                .badge()
                .map(|b| format!("  [{b}]"))
                .unwrap_or_default();
            let id_short: String = row.id.chars().take(16).collect();
            cprintln!(
                "  {}{}  {}{badge}  {}",
                row.prefix,
                id_short,
                row.created_at,
                preview
            );
        }
        CommandOutcome::Handled
    }
}
