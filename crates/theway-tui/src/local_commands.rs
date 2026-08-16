//! TUI-local slash commands and helpers — the client-local command surface:
//! `/quit`, `/clear`, `/help` plus the interactive login prompt. Runtime
//! commands run daemon-side.

use std::sync::Arc;

use async_trait::async_trait;
use theway_transport::commands::{CommandCtx, CommandOutcome, Registry, SlashCommand};

/// The TUI-local command set: everything that runs without a daemon round-trip.
pub fn local_registry() -> Registry {
    let mut r = Registry::new();
    r.register(Arc::new(HelpCommand));
    r.register(Arc::new(ClearCommand));
    r.register(Arc::new(QuitCommand));
    r
}

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

/// Prompt the user for a provider API key on the terminal (no echo). Errors when
/// stdin is not a TTY. Used by the TUI's `/login` flow; the daemon picks the key
/// up from the shared auth store on its next turn (no protocol change).
pub async fn prompt_for_api_key(provider: &str) -> anyhow::Result<String> {
    use anyhow::Context as _;
    use std::io::IsTerminal as _;
    let provider = provider.to_string();
    tokio::task::spawn_blocking(move || {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(theway_transport::auth::login_requires_tty_message(
                &provider, None
            ));
        }
        rpassword::prompt_password(format!("api key for `{provider}`: "))
            .context("read api key without echo")
    })
    .await
    .context("login prompt task")?
}

/// Helper for tests / prompt construction: a raw llm-provider user message.
pub fn user_message(text: &str) -> theway_core::AgentMessage {
    theway_core::AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: chrono::Utc::now().timestamp_millis(),
        },
    ))
}
