//! Startup orchestration for the `theway` client binary: find a running
//! `thewayd` (port file → default port probe), spawn one when absent, wait for
//! readiness, fetch the initial snapshot, and hand off to the client App.
//!
//! The old in-process assembly (harness / session / DAG / MCP / skills /
//! triggers) moved to the daemon (`theway-server/src/bin/thewayd.rs`) — this
//! file no longer constructs any runtime state.

use anyhow::Result;
use theway_contract::session::SessionReader;
use theway_storage::session;
use theway_storage::sqlite_repo::SqliteSessionRepo;
use theway_transport::wire::WireDaemonConfig;

use crate::cli::Cli;
use crate::ui;

mod connection;
pub(crate) use connection::DaemonConnector;

/// Map the client CLI to daemon launch arguments (design decision 3: session
/// selection is a daemon launch concern when the TUI spawns it).
///
/// Issue #74: the config-shaped flags (model, builtin skills, trigger poll
/// interval) are emitted from the assembled config payload, not the raw CLI
/// struct — the payload carries the CLI values PLUS the local `config.toml`
/// values (the daemon no longer reads that file itself since #73).
/// `--base-url` stays CLI-driven; `--thinking` comes from the CLI flag when
/// given, otherwise from the persisted `[model] thinking` level in the
/// assembled config payload (the user's last pick).
fn daemon_launch_args(cli: &Cli, config: &WireDaemonConfig) -> Vec<String> {
    let mut args = Vec::new();
    if cli.continue_ {
        args.push("--continue".to_string());
    }
    if let Some(id) = cli.effective_resume_id() {
        args.push("--resume-id".to_string());
        args.push(id.to_string());
    }
    args.extend(daemon_runtime_args(cli, config));
    args
}

/// Daemon arguments that are stable across initial spawn and recovery. Session
/// selection is deliberately excluded so reconnect can prepend the App's
/// authoritative current session id.
fn daemon_runtime_args(cli: &Cli, config: &WireDaemonConfig) -> Vec<String> {
    let mut args = Vec::new();
    // Model is session-level, injected by the client per-session via `SetModel`
    // (through the settings/Configure path). The daemon no longer accepts or
    // resolves a startup model, so `--provider` / `--model` are NOT passed.
    if let Some(base_url) = &cli.base_url {
        args.push("--base-url".to_string());
        args.push(base_url.clone());
    }
    match cli.thinking.as_deref() {
        // Explicit flag (any level, including off) wins over the persisted
        // `[model] thinking` default in the payload.
        Some(level) => {
            args.push("--thinking".to_string());
            args.push(level.to_string());
        }
        // No flag: the persisted last-pick level from config.toml.
        None => {
            if let Some(level) = config.thinking_level.as_deref() {
                args.push("--thinking".to_string());
                args.push(level.to_string());
            }
        }
    }
    if cli.yes {
        args.push("--yes".to_string());
    }
    if cli.always_allow {
        args.push("--always-allow".to_string());
    }
    for skill in &config.builtin_skills {
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
    if let Some(secs) = config.trigger_poll_secs {
        args.push("--trigger-poll-secs".to_string());
        args.push(secs.to_string());
    }
    if let Some(addr) = &config.storage_service_addr {
        args.push("--storage-service-addr".to_string());
        args.push(addr.clone());
    }
    if cli.debug {
        args.push("--debug".to_string());
    }
    args
}

/// Fresh-attach gate (issue #56): the TUI defaults to a new session only
/// when it REUSED a running daemon (`reused`) and the user passed no
/// explicit session selection — `--resume` (bare picker or id),
/// `--resume-id`, or `--continue`. A self-spawned daemon already has a
/// fresh session, so `reused = false` never attaches fresh; the daemon is
/// untouched either way (fresh attach is TUI client semantics).
///
/// Issue #46: the actual `create_session` is deferred until the first
/// submitted message (`App::ensure_fresh_session`) — the flag only arms the
/// client.
fn fresh_attach_wanted(reused: bool, cli: &Cli) -> bool {
    reused && cli.resume.is_none() && cli.resume_id.is_none() && !cli.continue_
}

/// Issue #47: `--continue` onto a reused daemon whose current session no
/// longer exists in the repo (a previous idle run reaped its empty startup
/// session and no session remained) must attach fresh instead of landing on
/// the deleted id — messages there would be silently lost.
fn continue_needs_fresh_attach(
    reused: bool,
    cli: &Cli,
    initial_session_id: &str,
    exists_in_repo: bool,
) -> bool {
    reused && cli.continue_ && !initial_session_id.is_empty() && !exists_in_repo
}

/// Issue #47: session id the SPAWNED daemon created at startup
/// (`SessionSelection::New`) — the TUI reaps it on exit when no message ever
/// reached it, so an idle TUI leaves no empty conversation behind. Explicit
/// selections never make the daemon create a session (`--continue` and
/// resume launch args); the reused-daemon path defers creation to the first
/// message (issue #46) and yields `None` here.
fn spawn_auto_session(reused: bool, cli: &Cli, initial_session_id: &str) -> Option<String> {
    if !reused
        && !cli.continue_
        && cli.effective_resume_id().is_none()
        && !initial_session_id.is_empty()
    {
        Some(initial_session_id.to_string())
    } else {
        None
    }
}

pub(crate) async fn run_repl(
    mut cli: Cli,
    cwd: std::path::PathBuf,
    repo: SqliteSessionRepo,
) -> Result<()> {
    let repo = std::sync::Arc::new(repo);

    // Issue #90: materialize the bundled TUI documentation before anything
    // else, so the `tui-docs` extension's prompt pointer always resolves —
    // regardless of install method — for both interactive and headless runs.
    crate::tui_docs::ensure_installed(&theway_transport::config::base_dir());

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
    // DaemonConnector also provisions the daemon config through the settings
    // RPC; `config_notes` carries assembly diagnostics and attach-time
    // mismatch reports for the feed.
    let (connector, connection) = DaemonConnector::start(&cli, &cwd, repo.clone()).await?;
    let client = connection.client;
    let initial = connection.status;
    let reused = connection.reused;
    let config_notes = connection.notes;

    // Issue #56: re-entering the TUI with a live daemon defaults to a NEW
    // session (daemon untouched — fresh attach is TUI client semantics).
    // Explicit `--resume`/`--resume-id`/`--continue` selections skip this.
    // Issue #46: the create is DEFERRED until the first submitted message —
    // an idle TUI must not leave an empty conversation behind. Until then the
    // client shows the daemon's current session; `App::ensure_fresh_session`
    // creates + selects the fresh session right before the first send.
    //
    // Issue #47: a reused daemon may hold a DELETED session as its active
    // runtime (a previous idle run reaped its empty startup session and no
    // session remained). `--continue` would otherwise attach to the deleted
    // id and silently lose messages — treat it like a fresh attach instead.
    let continue_target_gone = {
        let exists = session::find_path_by_id(&repo, &initial.session_id)
            .await?
            .is_some();
        continue_needs_fresh_attach(reused, &cli, &initial.session_id, exists)
    };
    let fresh_attach = fresh_attach_wanted(reused, &cli) || continue_target_gone;

    // Issue #47: a SPAWNED daemon creates its startup session eagerly
    // (SessionSelection::New — the runtime needs a session). The TUI reaps
    // that session on exit when no message ever reached it, so an idle TUI
    // leaves no empty conversation behind. Explicit selections never make the
    // daemon create a session (`--continue`/resume launch args), and the
    // reused-daemon path already defers creation to the first message (#46).
    let auto_session = spawn_auto_session(reused, &cli, &initial.session_id);

    let session_id = if session_id.is_empty() {
        initial.session_id.clone()
    } else {
        session_id
    };
    let _ = session_id; // local surfaces read the id from the snapshot's session

    let mut app = ui::App::new(ui::AppConfig {
        client,
        connector: Some(connector),
        initial,
        cwd: cwd.clone(),
        model_config_path: crate::config_payload::config_path(cli.home.as_deref()),
        history: theway_transport::history::HistoryStore::load(),
        registry: crate::local_commands::local_registry(),
        pending_images: cli.image.clone(),
        color_level: theway_markdown::get_color_level(),
        fresh_attach,
        auto_session,
    });
    // Issue #97: on a fresh attach the new session is created eagerly — the
    // panel, stream and snapshot point at it from the first frame, so the
    // previous session is never visible as "current". The fresh id is also
    // marked as the auto session: an idle exit reaps it (issue #47), keeping
    // the no-empty-conversation invariant from issue #46.
    if fresh_attach {
        match app.ensure_fresh_session().await {
            Ok(id) => {
                app.set_auto_session(id.clone());
                app.system_line(format!(
                    "attached to a running daemon; created fresh session {id} (previous session not attached)"
                ));
            }
            Err(error) => {
                app.system_line(format!("fresh session create failed: {error}"));
            }
        }
    }
    app.banner();
    app.system_line(format!(
        "connected to daemon at {} (cwd: {})",
        app.client_addr(),
        cwd.display()
    ));
    // Issue #74: surface config assembly diagnostics + attach-time mismatch
    // reports (values the running daemon cannot re-apply at runtime).
    for note in config_notes {
        app.system_line(note);
    }
    app.run().await
}

async fn session_id_of(session: &(impl SessionReader + ?Sized)) -> String {
    session
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
    use theway_transport::client::GrpcClient;
    use theway_transport::grpc::{GrpcState, serve_grpc};
    use theway_transport::testing::{
        ChannelCommandOps, FakeSessionOps, LiveSessionObservability, SharedSettingsOps,
    };
    use theway_transport::wire::{WireCommand, WireDaemonConfig, WirePathContext, WireStatus};
    use tokio::sync::{broadcast, mpsc};

    pub(crate) fn test_status() -> WireStatus {
        WireStatus {
            session_id: "sess-1".into(),
            model: "provider:model".into(),
            thinking_level: "off".into(),
            model_catalog: Vec::new(),
            cwd: "/tmp/theway".into(),
            busy: false,
            queued_count: 0,
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            sidebar: theway_transport::testing::empty_sidebar_snapshot(),
            feed_blocks: Vec::new(),
            feed_blocks_base: 0,
            feed_block_patches: Vec::new(),
            feed_lines: Vec::new(),
            feed_lines_base: 0,
            dags: Vec::new(),
            subagents: Vec::new(),
            usage: theway_transport::wire::WireContextUsage::default(),
            session_usage: theway_transport::wire::WireContextUsage::default(),
            tui_max_feed_lines: None,
            extensions: theway_transport::wire::WireExtensionSnapshot::default(),
            system_context: String::new(),
            shell_count: 0,
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
        let (snapshot_tx, _) = broadcast::channel::<theway_transport::wire::WireStatusUpdate>(16);
        let latest = Arc::new(parking_lot::Mutex::new(test_status()));
        let (event_tx, _) = broadcast::channel::<theway_transport::wire::WireAgentEvent>(16);
        let (dag_event_tx, _) = broadcast::channel::<theway_transport::wire::WireDagEvent>(16);
        let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
        let session_ops = Arc::new(FakeSessionOps::new());
        for id in seeds {
            session_ops.add_session(id);
        }
        let current: String = seeds.first().copied().unwrap_or("").to_string();
        let path_context = Arc::new(std::sync::RwLock::new(WirePathContext::default()));
        let daemon_config = Arc::new(std::sync::RwLock::new(WireDaemonConfig::default()));
        let session_states = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
        let external_ops: Arc<dyn theway_transport::ExternalProtocolOps> =
            Arc::new(theway_transport::CompositeExternalProtocolOps::new(
                Arc::new(ChannelCommandOps::new(command_tx.clone())),
                session_ops.clone(),
                Arc::new(LiveSessionObservability::new(
                    session_ops.clone(),
                    session_states.clone(),
                    latest.clone(),
                    current.clone(),
                )),
                Arc::new(theway_transport::UnavailableGraphOps),
                Arc::new(theway_transport::UnavailableToolOps),
                Arc::new(theway_transport::UnavailableStorageOps),
                Arc::new(SharedSettingsOps::new(
                    path_context.clone(),
                    daemon_config.clone(),
                    command_tx.clone(),
                )),
            ));
        let state = GrpcState {
            commands: command_tx,
            snapshots: snapshot_tx,
            latest,
            session_states,
            events: event_tx,
            dag_events: dag_event_tx,
            job_ops: Arc::new(theway_transport::UnavailableJobOps),
            graph_ops: Arc::new(theway_transport::UnavailableGraphOps),
            session_ops: session_ops.clone(),
            session_id: Arc::new(std::sync::RwLock::new(current)),
            agent_fwd,
            path_context,
            daemon_config,
            tool_ops: Arc::new(theway_transport::UnavailableToolOps),
            storage_ops: Arc::new(theway_transport::UnavailableStorageOps),
            external_ops,
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
    use super::*;
    use clap::Parser as _;

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

    /// Issue #47: `--continue` onto a reused daemon attaches fresh only when
    /// the daemon's current session no longer exists in the repo (reaped by
    /// an idle previous run). An existing target keeps the resume semantics.
    #[test]
    fn continue_target_gone_triggers_fresh_attach_only_for_missing_session() {
        let plain = Cli::parse_from(["theway", "--continue"]);
        assert!(!continue_needs_fresh_attach(true, &plain, "sess-1", true));
        assert!(continue_needs_fresh_attach(true, &plain, "sess-1", false));
        // An empty snapshot id never triggers (nothing to resolve).
        assert!(!continue_needs_fresh_attach(true, &plain, "", false));
        // The spawn path never fresh-attaches; `--continue` resumes server-side.
        assert!(!continue_needs_fresh_attach(false, &plain, "sess-1", false));
    }

    /// Issue #47: the SPAWNED daemon's startup session is the reaping target
    /// only with no explicit selection; `--continue`/resume launch args make
    /// the daemon resume instead of create, and reused daemons defer creation
    /// to the first message (issue #46).
    #[test]
    fn spawn_auto_session_only_for_plain_spawn() {
        let plain = Cli::parse_from(["theway"]);
        assert_eq!(
            spawn_auto_session(false, &plain, "sess-new").as_deref(),
            Some("sess-new")
        );
        assert_eq!(spawn_auto_session(false, &plain, ""), None);

        for args in [
            vec!["theway", "--continue"],
            vec!["theway", "--resume-id", "sess-abc"],
            vec!["theway", "--resume", "sess-abc"],
        ] {
            let cli = Cli::parse_from(args.clone());
            assert_eq!(
                spawn_auto_session(false, &cli, "sess-new"),
                None,
                "explicit selection must not create a startup session: {args:?}"
            );
        }

        // Reused daemon: nothing was created at startup — the fresh attach is
        // deferred to the first message.
        let cli = Cli::parse_from(["theway"]);
        assert_eq!(spawn_auto_session(true, &cli, "sess-old"), None);
    }

    /// Issue #66: `--home` (when set) and each repeatable `--skills-dir`
    /// forward into the daemon launch args as their own flag/value pairs, in
    /// CLI order; both are omitted entirely when unset (the daemon then
    /// resolves the home from the environment itself).
    #[test]
    fn daemon_launch_args_forwards_home_and_skills_dirs() {
        // Unset: neither flag appears in the launch args.
        let plain = Cli::parse_from(["theway"]);
        let args = daemon_launch_args(&plain, &WireDaemonConfig::default());
        assert!(!args.iter().any(|a| a == "--home"));
        assert!(!args.iter().any(|a| a == "--skills-dir"));
        assert!(!args.iter().any(|a| a == "--storage-service-addr"));

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
        let (config, _) = crate::config_payload::assemble_config_from(
            &cli,
            None,
            "config.toml",
            std::path::Path::new("/tmp/fake-cwd"),
        );
        assert_eq!(
            daemon_launch_args(&cli, &config)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
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

    #[test]
    fn daemon_runtime_args_leave_session_selection_to_recovery() {
        let cli = Cli::parse_from([
            "theway",
            "--continue",
            "--provider",
            "openai",
            "--model",
            "gpt-test",
        ]);
        let config = WireDaemonConfig {
            provider: cli.provider.clone(),
            model: cli.model.clone(),
            ..WireDaemonConfig::default()
        };

        let args = daemon_runtime_args(&cli, &config);
        assert!(!args.iter().any(|arg| arg == "--continue"));
        assert!(!args.iter().any(|arg| arg == "--resume-id"));
        // The daemon no longer accepts a startup model; the session-level model
        // is injected by the client via Configure/SetModel after attach.
        assert!(args.is_empty());
    }

    /// Issue #74: the config-shaped launch args come from the ASSEMBLED
    /// payload — CLI flag values flow through it, and so do `config.toml`
    /// values the daemon can no longer read itself (`[model]` default,
    /// `[builtin_skills] enabled`, `[triggers] poll_interval_secs`).
    #[test]
    fn daemon_launch_args_carries_file_config_through_the_payload() {
        let toml = "\
[model]
provider = \"acme\"
model = \"warp-9\"
thinking = \"high\"

[builtin_skills]
enabled = [\"debugging\"]

[triggers]
poll_interval_secs = 45
";
        // No CLI config flags at all — every config launch arg is file-derived.
        let cli = Cli::parse_from(["theway"]);
        let (config, _) = crate::config_payload::assemble_config_from(
            &cli,
            Some(toml),
            "config.toml",
            std::path::Path::new("/tmp/fake-cwd"),
        );
        assert_eq!(
            daemon_launch_args(&cli, &config)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "--thinking",
                "high",
                "--builtin-skill",
                "debugging",
                "--trigger-poll-secs",
                "45",
            ]
        );

        // CLI flags win inside the payload (and therefore in the launch args);
        // file builtins union in after the CLI entries. The file's persisted
        // thinking level still flows through when no `--thinking` flag is given.
        let cli = Cli::parse_from([
            "theway",
            "--provider",
            "openai",
            "--model",
            "gpt-x",
            "--builtin-skill",
            "code-review",
            "--trigger-poll-secs",
            "15",
        ]);
        let (config, _) = crate::config_payload::assemble_config_from(
            &cli,
            Some(toml),
            "config.toml",
            std::path::Path::new("/tmp/fake-cwd"),
        );
        assert_eq!(
            daemon_launch_args(&cli, &config)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "--thinking",
                "high",
                "--builtin-skill",
                "code-review",
                "--builtin-skill",
                "debugging",
                "--trigger-poll-secs",
                "15",
            ]
        );

        // A CLI `--thinking` flag beats the file's persisted level.
        let cli = Cli::parse_from(["theway", "--thinking", "minimal"]);
        let (config, _) = crate::config_payload::assemble_config_from(
            &cli,
            Some(toml),
            "config.toml",
            std::path::Path::new("/tmp/fake-cwd"),
        );
        assert_eq!(
            daemon_launch_args(&cli, &config)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec![
                "--thinking",
                "minimal",
                "--builtin-skill",
                "debugging",
                "--trigger-poll-secs",
                "45",
            ]
        );
    }
}
