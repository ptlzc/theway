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
    // Issue #66: pass the user-level root and extra skill scan roots through
    // verbatim. Unset `--home` means no flag at all — the daemon resolves the
    // home from the environment itself at its CLI boundary.
    if let Some(home) = &cli.home {
        args.push("--home".to_string());
        args.push(home.display().to_string());
    }
    for dir in &cli.skills_dir {
        args.push("--skills-dir".to_string());
        args.push(dir.display().to_string());
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

/// Find a daemon or spawn one, then return a connected client + initial
/// state + a `reused` marker (issue #56): `true` only when the client
/// attached to an already-running daemon via `discover`. The marker drives
/// the fresh-attach step in `run_repl` — a freshly spawned daemon
/// (`reused = false`) already starts on its own new session.
async fn connect_or_spawn(
    cli: &Cli,
    cwd: &std::path::Path,
) -> Result<(GrpcClient, WireStatus, bool)> {
    // 1. Reuse a running daemon: per-cwd port file first, default port second.
    if let Some(addr) = discover(std::time::Duration::from_millis(800), cwd).await? {
        tracing::info!("reusing running daemon at {addr}");
        let mut client = GrpcClient::connect(&addr).await?;
        let state = client.get_state().await?;
        return Ok((client, wire_status(&state), true));
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
    Ok((client, wire_status(&state), false))
}

/// Fresh-attach gate (issue #56): the TUI defaults to a new session only
/// when it REUSED a running daemon (`reused`) and the user passed no
/// explicit session selection — `--resume` (bare picker or id),
/// `--resume-id`, or `--continue`. A self-spawned daemon already has a
/// fresh session, so `reused = false` never attaches fresh; the daemon is
/// untouched either way (fresh attach is TUI client semantics).
fn fresh_attach_wanted(reused: bool, cli: &Cli) -> bool {
    reused && cli.resume.is_none() && cli.resume_id.is_none() && !cli.continue_
}

/// Create + switch to a new session after attaching to a reused daemon —
/// the same path `/new` drives (issue #56). Returns the new session id.
/// The switch queues on the daemon's serialized event loop, so the current
/// snapshot may still describe the old session until the republish lands.
async fn attach_fresh_session(client: &mut GrpcClient) -> Result<String> {
    let summary = client.create_session(None).await?;
    let id = summary.session_id.clone();
    if !client.switch_session(&id).await? {
        anyhow::bail!("daemon rejected the session switch");
    }
    Ok(id)
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

    // Connect (reuse or spawn), fetch the initial snapshot. `reused` marks a
    // discover hit (issue #56): an already-running daemon would resume its
    // OLD session, so the fresh-attach step below applies. A freshly
    // spawned daemon (`reused = false`) already starts on its own new
    // session — creating another would leave an extra empty session behind.
    let (mut client, mut initial, reused) = connect_or_spawn(&cli, &cwd).await?;

    // Issue #56: re-entering the TUI with a live daemon defaults to a NEW
    // session (daemon untouched — fresh attach is TUI client semantics).
    // Explicit `--resume`/`--resume-id`/`--continue` selections skip this.
    // Mirrors the /new path: create + switch. The switch queues on the
    // daemon's event loop, so the first snapshots may still show the old
    // session; the new one appears on the next snapshot (apply_snapshot's
    // session-id path).
    let fresh_session_id = if fresh_attach_wanted(reused, &cli) {
        let id = attach_fresh_session(&mut client).await?;
        // Patch the cached snapshot's session id so the banner names the
        // new session before the first republish lands (the feed still
        // holds the old session's blocks until then).
        initial.session_id = id.clone();
        Some(id)
    } else {
        None
    };

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
    if let Some(id) = fresh_session_id {
        app.system_line(format!("new session {id}"));
    }
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

/// Shared in-process gRPC daemon fixture for the TUI unit tests (issue #64):
/// a real tonic server over [`FakeSessionOps`] on a random loopback port, so
/// client-side code round-trips through real frames without touching a live
/// daemon or the local session repo. The mock command channel records every
/// daemon-side [`WireCommand`] for sequence assertions.
#[cfg(test)]
pub(crate) mod test_daemon {
    use std::sync::Arc;
    use theway_core::multiagent::graph::engine::DagEngine;
    use theway_core::multiagent::graph::types::DagEvent;
    use theway_core::multiagent::registry::{AgentJobEvent, AgentJobRegistry};
    use theway_transport::client::GrpcClient;
    use theway_transport::grpc::{GrpcState, serve_grpc};
    use theway_transport::testing::FakeSessionOps;
    use theway_transport::wire::{WireCommand, WireStatus};
    use tokio::sync::{broadcast, mpsc};

    pub(crate) fn test_status() -> WireStatus {
        WireStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: theway_transport::testing::empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_lines: Vec::new(),
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: theway_transport::wire::WireContextUsage::default(),
            tui_max_feed_lines: None,
        }
    }

    /// In-process gRPC fixture (the same `GrpcState` shape the UI tests
    /// use) with an explicit seed session list; the first seed becomes the
    /// daemon's current session. Returns the connected client, the command
    /// recorder, and the `FakeSessionOps` handle (delete-protection setup).
    pub(crate) async fn test_daemon_client_with_sessions(
        seeds: &[&str],
    ) -> (
        GrpcClient,
        mpsc::UnboundedReceiver<WireCommand>,
        Arc<FakeSessionOps>,
    ) {
        let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
        let (snapshot_tx, _) = broadcast::channel::<WireStatus>(16);
        let latest = Arc::new(parking_lot::Mutex::new(test_status()));
        let (event_tx, _) = broadcast::channel::<AgentJobEvent>(16);
        let (dag_event_tx, _) = broadcast::channel::<DagEvent>(16);
        let registry = AgentJobRegistry::new();
        let agent_fwd = {
            let mut rx = registry.subscribe();
            let fwd_tx = event_tx.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let _ = fwd_tx.send(event);
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("AgentJobEvent broadcast lagged by {n}, skipping");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            })
        }
        .abort_handle();
        let session_ops = Arc::new(FakeSessionOps::new());
        for id in seeds {
            session_ops.add_session(id);
        }
        let current: String = seeds.first().copied().unwrap_or("").to_string();
        let state = GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            events: event_tx,
            dag_events: dag_event_tx,
            registry,
            dag_engine: Arc::new(DagEngine::new()),
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new(current)),
            agent_fwd,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = serve_grpc(listener, state);
        let _server = server;
        let client = GrpcClient::connect(&addr).await.unwrap();
        (client, command_rx, session_ops)
    }

    /// [`test_daemon_client_with_sessions`] seeded with one session
    /// (`sess-1`, current) — the common single-session shape.
    pub(crate) async fn test_daemon_client() -> (
        GrpcClient,
        mpsc::UnboundedReceiver<WireCommand>,
        Arc<FakeSessionOps>,
    ) {
        test_daemon_client_with_sessions(&["sess-1"]).await
    }
}

#[cfg(test)]
mod tests {
    use super::test_daemon::test_daemon_client;
    use super::*;
    use clap::Parser as _;
    use theway_transport::wire::WireCommand;

    /// Issue #56 gate: only a REUSED daemon with no explicit session
    /// selection (`--resume` bare or with an id, `--resume-id`,
    /// `--continue`) triggers the fresh-attach step. The spawn path
    /// (`reused = false`) never does — the spawned daemon already starts on
    /// its own new session — and neither does any resume/continue flag.
    #[test]
    fn fresh_attach_gate_reused_without_resume_flags_only() {
        let plain = Cli::parse_from(["theway"]);
        assert!(fresh_attach_wanted(true, &plain));
        assert!(
            !fresh_attach_wanted(false, &plain),
            "a spawned daemon already has a fresh session"
        );

        for args in [
            vec!["theway", "--resume"],
            vec!["theway", "--resume", "sess-abc"],
            vec!["theway", "--resume-id", "sess-abc"],
            vec!["theway", "--continue"],
        ] {
            let cli = Cli::parse_from(args.clone());
            assert!(
                !fresh_attach_wanted(true, &cli),
                "explicit selection must suppress fresh attach: {args:?}"
            );
        }
    }

    /// Issue #66: `--home` (when set) and each repeatable `--skills-dir`
    /// forward into the daemon launch args as their own flag/value pairs, in
    /// CLI order; both are omitted entirely when unset (the daemon then
    /// resolves the home from the environment itself).
    #[test]
    fn daemon_launch_args_forwards_home_and_skills_dirs() {
        // Unset: neither flag appears in the launch args.
        let plain = Cli::parse_from(["theway"]);
        let args = daemon_launch_args(&plain);
        assert!(!args.iter().any(|a| a == "--home"));
        assert!(!args.iter().any(|a| a == "--skills-dir"));

        // Set: `--home` becomes one pair; every `--skills-dir` occurrence
        // becomes its own pair, preserving CLI order.
        let cli = Cli::parse_from([
            "theway",
            "--provider",
            "acme",
            "--home",
            "/tmp/fake-home",
            "--skills-dir",
            "/tmp/skills-a",
            "--skills-dir",
            "/tmp/skills-b",
            "--debug",
        ]);
        assert_eq!(
            cli.home.as_deref(),
            Some(std::path::Path::new("/tmp/fake-home"))
        );
        assert_eq!(cli.skills_dir.len(), 2);
        assert_eq!(
            daemon_launch_args(&cli)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "--provider",
                "acme",
                "--home",
                "/tmp/fake-home",
                "--skills-dir",
                "/tmp/skills-a",
                "--skills-dir",
                "/tmp/skills-b",
                "--debug",
            ]
        );
    }

    /// Issue #56 reused path: `attach_fresh_session` runs the `/new` path —
    /// `create_session(None)` then `switch_session(id)` — and the mock
    /// command channel sees both, in order, carrying the new session id.
    /// (The gRPC create handler itself enqueues the first switch — becoming
    /// current is serialized through the event loop — and the client-side
    /// call enqueues the second.)
    #[tokio::test]
    async fn attach_fresh_session_creates_then_switches_in_order() {
        let (mut client, mut rx, _session_ops) = test_daemon_client().await;

        // Act
        let id = attach_fresh_session(&mut client).await.unwrap();

        // Assert: FakeSessionOps ids come from a counter — the first create
        // yields `sess-new-1`; both switches carry it, in command order.
        assert_eq!(id, "sess-new-1");
        for (i, origin) in ["create-time switch", "client-side switch"]
            .iter()
            .enumerate()
        {
            let cmd = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("no switch_session command")
                .unwrap();
            match cmd {
                WireCommand::SwitchSession { id: got } => {
                    assert_eq!(got, "sess-new-1", "wrong id after {origin} (index {i})")
                }
                other => panic!("unexpected command after {origin} (index {i}): {other:?}"),
            }
        }

        // Assert: the new session is registered and current daemon-side.
        let (sessions, current) = client.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "sess-1");
        assert_eq!(sessions[1].session_id, "sess-new-1");
        assert_eq!(current, "sess-new-1");
    }
}
