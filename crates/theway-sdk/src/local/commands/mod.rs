//! Local slash commands — the command set that needs no daemon runtime:
//! `/quit`, `/clear`, `/help`, `/login`, `/logout`, `/sessions`.
//!
//! Split out of the daemon crate (sdk-split-local-sandbox, node 5-commands-layer).
//! Every command implements [`SlashCommand<X>`] for *all* `X`, so any registry —
//! the SDK's offline [`Registry::local`] set or the daemon's runtime registry with
//! its own context extras (node 6: `DaemonCtx`) — can register them.
//!
//! Offline semantics: these commands run entirely from the SDK surface (auth store,
//! session repo, harness snapshot) with no daemon round-trip, matching the
//! `Command registry layering` requirement in specs/sdk/layout.

use std::sync::Arc;

use async_trait::async_trait;

use theway_transport::commands::{CommandCtx, CommandOutcome, Registry, SlashCommand};

pub struct QuitCommand;

#[async_trait]
impl<X: Send + Sync> SlashCommand<X> for QuitCommand {
    fn name(&self) -> &'static str {
        "quit"
    }
    fn aliases(&self) -> &'static [&'static str] {
        &["exit", "q"]
    }
    fn description(&self) -> &'static str {
        "exit the REPL"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, X>) -> CommandOutcome {
        CommandOutcome::Quit
    }
}

pub struct ClearCommand;

#[async_trait]
impl<X: Send + Sync> SlashCommand<X> for ClearCommand {
    fn name(&self) -> &'static str {
        "clear"
    }
    fn description(&self) -> &'static str {
        "clear screen (keeps conversation history)"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, X>) -> CommandOutcome {
        CommandOutcome::ClearScreen
    }
}

pub struct HelpCommand;

#[async_trait]
impl<X: Send + Sync> SlashCommand<X> for HelpCommand {
    fn name(&self) -> &'static str {
        "help"
    }
    fn description(&self) -> &'static str {
        "show available commands and model catalog help"
    }
    fn usage(&self) -> &'static str {
        "[models|<command>]"
    }
    async fn run(&self, _argv: &[String], _ctx: &CommandCtx<'_, X>) -> CommandOutcome {
        // Help needs the Registry itself to enumerate commands, which commands don't
        // receive; hosts special-case `/help` before dispatch (the daemon's `dispatch`
        // renders it from the registry). The registered entry keeps `/help` discoverable
        // in listings and completion.
        CommandOutcome::Handled
    }
}

pub struct LoginCommand;

#[async_trait]
impl<X: Send + Sync> SlashCommand<X> for LoginCommand {
    fn name(&self) -> &'static str {
        "login"
    }
    fn description(&self) -> &'static str {
        "store an API key for a provider in ~/.theway/auth.json"
    }
    fn usage(&self) -> &'static str {
        "<provider>"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, X>) -> CommandOutcome {
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
impl<X: Send + Sync> SlashCommand<X> for LogoutCommand {
    fn name(&self) -> &'static str {
        "logout"
    }
    fn description(&self) -> &'static str {
        "remove a stored credential from ~/.theway/auth.json"
    }
    fn usage(&self) -> &'static str {
        "<provider>"
    }
    async fn run(&self, argv: &[String], _ctx: &CommandCtx<'_, X>) -> CommandOutcome {
        if argv.is_empty() {
            return CommandOutcome::Error("usage: /logout <provider>".into());
        }
        let provider = &argv[0];
        let mut store = match crate::local::auth::AuthStore::load() {
            Ok(s) => s,
            Err(e) => return CommandOutcome::Error(format!("load auth store: {e}")),
        };
        match store.remove(provider) {
            Some(_) => match store.save() {
                Ok(()) => {
                    crate::cprintln!("removed credential for `{provider}`");
                    CommandOutcome::Handled
                }
                Err(e) => CommandOutcome::Error(format!("save auth store: {e}")),
            },
            None => {
                crate::cprintln!("no credential stored for `{provider}`");
                CommandOutcome::Handled
            }
        }
    }
}

pub struct SessionsCommand;

#[async_trait]
impl<X: Send + Sync> SlashCommand<X> for SessionsCommand {
    fn name(&self) -> &'static str {
        "sessions"
    }
    fn description(&self) -> &'static str {
        "list sessions for this cwd"
    }
    async fn run(&self, _argv: &[String], ctx: &CommandCtx<'_, X>) -> CommandOutcome {
        let repo = crate::local::session::open_repo(ctx.cwd).await;
        let entries = match crate::local::session::list_entries(&repo).await {
            Ok(e) => e,
            Err(e) => return CommandOutcome::Error(format!("list sessions: {e}")),
        };
        if entries.is_empty() {
            crate::cprintln!("(no sessions for this cwd)");
            return CommandOutcome::Handled;
        }
        crate::cprintln!("Sessions:");
        for e in entries {
            let preview = e.preview.as_deref().unwrap_or("");
            let id_short: String = e.id.chars().take(16).collect();
            crate::cprintln!("  {}  {}  {}", id_short, e.created_at, preview);
        }
        CommandOutcome::Handled
    }
}

/// Extension trait so the SDK can add constructors to the (transport-owned)
/// [`Registry`] without violating the orphan rule (daemon-kernel-layers: the
/// framework moved to transport; the local command set stays here until it
/// splits into tui / daemon).
pub trait RegistryLocalExt<X> {
    /// The local command set: everything that runs without a daemon runtime.
    /// Clients enumerate/dispatch these offline; the daemon appends its runtime
    /// commands on top.
    fn local() -> Registry<X>;

    /// Compatibility alias for [`RegistryLocalExt::local`] kept while the TUI
    /// still calls `Registry::with_builtins()`.
    fn with_builtins() -> Registry<X>;
}

impl<X: Send + Sync> RegistryLocalExt<X> for Registry<X> {
    fn local() -> Registry<X> {
        let mut r = Registry::new();
        r.register(Arc::new(HelpCommand));
        r.register(Arc::new(ClearCommand));
        r.register(Arc::new(QuitCommand));
        r.register(Arc::new(LoginCommand));
        r.register(Arc::new(LogoutCommand));
        r.register(Arc::new(SessionsCommand));
        r
    }

    fn with_builtins() -> Registry<X> {
        Self::local()
    }
}
