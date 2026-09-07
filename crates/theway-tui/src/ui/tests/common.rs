use super::*;

/// Process-unique history file so tests that append history entries never
/// pollute (or read) a shared path across test runs.
fn test_history_store() -> HistoryStore {
    static HISTORY_DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = HISTORY_DIR.get_or_init(|| tempfile::tempdir().unwrap());
    HistoryStore::load_from(&dir.path().join("history"))
}

/// Serializes tests that mutate `THEWAY_DIR`/`HOME` process-wide.
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn fixture_status(feed_blocks: Vec<WireFeedBlock>) -> WireStatus {
    WireStatus {
        session_id: "sess-1".into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: vec![theway_transport::wire::ProviderGroup {
            provider: "anthropic".into(),
            has_credential: true,
            models: vec![theway_transport::wire::ModelEntry {
                id: "claude-x".into(),
                name: "Claude X".into(),
            }],
        }],
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: theway_transport::testing::empty_sidebar_snapshot(),
        feed_blocks,
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: theway_transport::wire::WireExtensionSnapshot::default(),
        system_context: String::new(),
        shell_count: 0,
        observability: Default::default(),
    }
}

/// In-process gRPC fixture + App: the client drives a real server (the
/// same GrpcState shape the transport tests use), so submit/cancel/approve
/// round-trip through actual tonic frames.
pub(crate) async fn test_app() -> (App, mpsc::UnboundedReceiver<WireCommand>) {
    let (app, rx, _ops) = test_app_with_sessions(&["sess-1"], false).await;
    (app, rx)
}

/// [`test_app`] with an explicit seed session list (empty = a daemon with
/// no sessions) plus the `FakeSessionOps` handle for tests that inspect or
/// mutate the session table (issue #56). `fresh_attach` mirrors
/// `AppConfig::fresh_attach` (issue #79): when true, `App::new` leaves the
/// initial feed empty instead of seeding the previous session's messages.
/// [`test_app_with_sessions`] with an explicit daemon-config seed. The
/// returned handle lets tests flip `GetConfig` responses without touching the
/// gRPC plumbing.
pub(crate) async fn test_app_with_sessions_and_config(
    seeds: &[&str],
    fresh_attach: bool,
    seed_config: WireDaemonConfig,
) -> (
    App,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
    Arc<std::sync::RwLock<WireDaemonConfig>>,
) {
    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let (snapshot_tx, _) = broadcast::channel::<theway_transport::wire::WireStatusUpdate>(16);
    let latest = Arc::new(parking_lot::Mutex::new(fixture_status(Vec::new())));
    let (event_tx, _) = broadcast::channel::<theway_transport::wire::WireAgentEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<theway_transport::wire::WireDagEvent>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let session_ops = Arc::new(FakeSessionOps::new());
    for id in seeds {
        session_ops.add_session(id);
    }
    let current: String = seeds.first().copied().unwrap_or("").to_string();
    let session_states = Arc::new(parking_lot::Mutex::new(
        seeds
            .iter()
            .map(|id| {
                let mut status = fixture_status(Vec::new());
                status.session_id = (*id).to_string();
                ((*id).to_string(), status)
            })
            .collect::<HashMap<_, _>>(),
    ));
    let path_context = Arc::new(std::sync::RwLock::new(WirePathContext::default()));
    let daemon_config = Arc::new(std::sync::RwLock::new(seed_config));
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
        daemon_config: daemon_config.clone(),
        tool_ops: Arc::new(theway_transport::UnavailableToolOps),
        storage_ops: Arc::new(theway_transport::UnavailableStorageOps),
        external_ops,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = serve_grpc(listener, state);
    let _server = server;
    let client = GrpcClient::connect(&addr).await.unwrap();
    let initial = fixture_status(vec![WireFeedBlock::Plain {
        text: "banner".into(),
        level: theway_transport::feed::Level::Header,
        timestamp: None,
    }]);
    let app = App::new(AppConfig {
        client,
        connector: None,
        initial,
        cwd: std::path::PathBuf::from("/tmp/theway"),
        model_config_path: std::path::PathBuf::from("/nonexistent-theway-config/config.toml"),
        history: test_history_store(),
        registry: crate::local_commands::local_registry(),
        pending_images: vec![],
        color_level: theway_markdown::ColorLevel::TrueColor,
        ui_state: Some(Default::default()),
        fresh_attach,
        auto_session: None,
    });
    let mut app = app;
    // App::new loads the real `~/.theway/theme.toml`; force the default so
    // tests never depend on the machine's theme file (theme-specific tests
    // set `app.theme` explicitly).
    app.theme = super::theme::Theme::default();
    (app, command_rx, session_ops, daemon_config)
}

pub(crate) async fn test_app_with_sessions(
    seeds: &[&str],
    fresh_attach: bool,
) -> (
    App,
    mpsc::UnboundedReceiver<WireCommand>,
    Arc<FakeSessionOps>,
) {
    let (app, rx, ops, _config) =
        test_app_with_sessions_and_config(seeds, fresh_attach, WireDaemonConfig::default()).await;
    (app, rx, ops)
}

/// Drains the fixture's command channel on a background task and answers
/// oneshot RPCs (`SetModel` / `SetThinking`) so `client.set_model(...)` /
/// `client.set_thinking(...)` complete. The in-process gRPC server waits for
/// the oneshot response, so a test that both awaits the RPC and reads the
/// channel itself would deadlock — the drainer breaks that cycle.
/// Returns the task handle plus a label list of every command seen
/// (`SetModel(anthropic:claude-x)`, `SetThinking(high)`, `Submit(/model list)`).
pub(crate) fn drain_commands(
    mut rx: mpsc::UnboundedReceiver<WireCommand>,
) -> (
    tokio::task::JoinHandle<()>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let collector = seen.clone();
    let handle = tokio::spawn(async move {
        while let Some(command) = rx.recv().await {
            let label = match command {
                WireCommand::SetModel { spec, response, .. } => {
                    let _ = response.send(true);
                    format!("SetModel({spec})")
                }
                WireCommand::SetThinking {
                    level, response, ..
                } => {
                    let _ = response.send(true);
                    format!("SetThinking({level})")
                }
                WireCommand::Submit { text, .. } => format!("Submit({text})"),
                WireCommand::Abort { session_id } => format!("Abort({session_id})"),
                other => format!("{other:?}"),
            };
            collector.lock().unwrap().push(label);
        }
    });
    (handle, seen)
}

pub(crate) fn buffer_text(buf: &Buffer) -> String {
    let area = *buf.area();
    let mut rows = Vec::new();
    for y in 0..area.height {
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        rows.push(row.trim_end().to_string());
    }
    rows.join("\n")
}
