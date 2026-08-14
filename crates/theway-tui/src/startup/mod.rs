//! Startup orchestration for the `theway` client binary: find a running
//! `thewayd` (port file → default port probe), spawn one when absent, wait for
//! readiness, fetch the initial snapshot, and hand off to the client App.
//!
//! The old in-process assembly (harness / session / DAG / MCP / skills /
//! triggers) moved to the daemon (`theway-server/src/bin/thewayd.rs`) — this
//! file no longer constructs any runtime state.

use std::io::IsTerminal as _;

use anyhow::{Context as _, Result};
use theway_storage::session;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::client::{GrpcClient, discover, spawn_daemon, wait_ready};
use theway_transport::proto::wire_status;
use theway_transport::wire::WireStatus;

use crate::cli::Cli;
use crate::ui;

/// Re-exported for `crate::user_message` compatibility (see `main.rs`); the
/// client path no longer uses it directly.
pub use crate::local_commands::user_message;

/// Map the client CLI to daemon launch arguments (design decision 3: session
/// selection is a daemon launch concern when the TUI spawns it).
fn daemon_launch_args(cli: &Cli) -> Vec<String> {
    let mut args = Vec::new();
    if cli.continue_ {
        args.push("--continue".to_string());
    }
    if let Some(id) = cli.effective_resume_id() {
        args.push("--resume-id".to_string());
        args.push(id.to_string());
    }
    if let Some(provider) = &cli.provider {
        args.push("--provider".to_string());
        args.push(provider.clone());
    }
    if let Some(model) = &cli.model {
        args.push("--model".to_string());
        args.push(model.clone());
    }
    if let Some(base_url) = &cli.base_url {
        args.push("--base-url".to_string());
        args.push(base_url.clone());
    }
    if cli.thinking != "off" {
        args.push("--thinking".to_string());
        args.push(cli.thinking.clone());
    }
    if cli.yes {
        args.push("--yes".to_string());
    }
    if cli.always_allow {
        args.push("--always-allow".to_string());
    }
    for skill in &cli.builtin_skill {
        args.push("--builtin-skill".to_string());
        args.push(skill.clone());
    }
    if let Some(secs) = cli.trigger_poll_secs {
        args.push("--trigger-poll-secs".to_string());
        args.push(secs.to_string());
    }
    if cli.debug {
        args.push("--debug".to_string());
    }
    args
}

/// Find a daemon or spawn one, then return a connected client + initial state.
async fn connect_or_spawn(cli: &Cli, cwd: &std::path::Path) -> Result<(GrpcClient, WireStatus)> {
    // 1. Reuse a running daemon: per-cwd port file first, default port second.
    if let Some(addr) = discover(std::time::Duration::from_millis(800), cwd).await? {
        tracing::info!("reusing running daemon at {addr}");
        let mut client = GrpcClient::connect(&addr).await?;
        let state = client.get_state().await?;
        return Ok((client, wire_status(&state)));
    }

    // 2. Spawn `thewayd` on demand (inherits cwd/env; `--port 0` publishes the
    //    actual port to the port file).
    if !std::io::stdin().is_terminal() {
        // No TTY: the user cannot see daemon logs; still spawn (detached
        // stdout/stderr inherit so diagnostics stay visible on the pipe).
    }
    let args = daemon_launch_args(cli);
    let mut child =
        spawn_daemon(cwd, &args).with_context(|| format!("spawn thewayd in {}", cwd.display()))?;
    let addr = match wait_ready(std::time::Duration::from_secs(20), cwd, child.id()).await {
        Ok(addr) => addr,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
    };
    tracing::info!("spawned daemon at {addr}");
    let mut client = GrpcClient::connect(&addr).await?;
    let state = client.get_state().await?;
    Ok((client, wire_status(&state)))
}

pub(crate) async fn run_repl(
    mut cli: Cli,
    cwd: std::path::PathBuf,
    repo: SqliteSessionRepo,
) -> Result<()> {
    let repo = std::sync::Arc::new(repo);

    // Resolve/create the session locally so CLI surfaces (--list-sessions,
    // export/import, delete) and the banner agree with the daemon's pick. The
    // daemon resolves its own session from the launch args; the TUI only needs
    // the id for local surfaces. When no explicit selection is given, create a
    // fresh session *in the repo* — the daemon creates its own when spawned, so
    // this local create is only a fallback for reuse-across-machines edge cases.
    let session_id = match cli.effective_resume_id() {
        Some(id) => {
            let Some(path) = session::find_path_by_id(&repo, id).await? else {
                anyhow::bail!("no session matches id {id}");
            };
            let session = repo.open(&path).await?;
            session_id_of(&session).await
        }
        // Bare `--resume` opens the local picker; the chosen id becomes a
        // daemon launch argument (`--resume-id`).
        None if cli.resume.is_some() => {
            let (session, resumed) = crate::cli::select_resume_session(&repo, &cwd).await?;
            let id = session_id_of(&session).await;
            if resumed {
                cli.resume = None;
                cli.resume_id = Some(id.clone());
            }
            // A "clean" picker choice (new session) needs no launch arg.
            id
        }
        _ => String::new(),
    };

    // Connect (reuse or spawn), fetch the initial snapshot.
    let (client, initial) = connect_or_spawn(&cli, &cwd).await?;
    let session_id = if session_id.is_empty() {
        initial.session_id.clone()
    } else {
        session_id
    };
    let _ = session_id; // local surfaces read the id from the snapshot's session

    let mut app = ui::App::new(ui::AppConfig {
        client,
        initial,
        cwd: cwd.clone(),
        history: theway_transport::history::HistoryStore::load(),
        registry: crate::local_commands::local_registry(),
        pending_images: cli.image.clone(),
    });
    app.banner();
    app.system_line(format!(
        "connected to daemon at {} (cwd: {})",
        app.client_addr(),
        cwd.display()
    ));
    app.run().await
}

async fn session_id_of(session: &theway_core::Session) -> String {
    session
        .storage()
        .get_metadata_json()
        .await
        .ok()
        .and_then(|m| m.get("id").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_default()
}
