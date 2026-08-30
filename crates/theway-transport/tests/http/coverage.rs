//! Focused coverage for HTTP transport branches not exercised by the main
//! endpoint/session/tool suites: direct dispatch paths, SSE lagged frames,
//! serve_web, and small JSON-RPC edge cases.

use super::super::*;
use crate::TransportEndpoints;
use crate::testing::{
    ChannelCommandOps, FakeSessionOps, FakeStorageOps, FakeToolOps, LiveSessionObservability,
    SharedSettingsOps, empty_sidebar_snapshot,
};
use crate::wire::{
    WireContextUsage, WireDagEvent, WireExtensionSnapshot, WireNodeOutput,
};
use serde_json::json;

fn status(session_id: &str) -> WireStatus {
    WireStatus {
        session_id: session_id.into(),
        model: "provider:model".into(),
        thinking_level: "off".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp/theway".into(),
        busy: false,
        queued_count: 0,
        latest_trigger_poll: None,
        goal: None,
        control_plane_prompt: None,
        sidebar: empty_sidebar_snapshot(),
        feed_blocks: Vec::new(),
        feed_blocks_base: 0,
        feed_block_patches: Vec::new(),
        feed_lines: Vec::new(),
        feed_lines_base: 0,
        dags: Vec::new(),
        subagents: Vec::new(),
        usage: WireContextUsage::default(),
        session_usage: WireContextUsage::default(),
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot::default(),
        system_context: String::new(),
    }
}

fn state_with(commands: mpsc::UnboundedSender<WireCommand>) -> HttpState {
    let session_ops: std::sync::Arc<dyn crate::transport::SessionOps> =
        std::sync::Arc::new(FakeSessionOps::new());
    let graph_ops: std::sync::Arc<dyn crate::GraphOps> =
        std::sync::Arc::new(crate::UnavailableGraphOps);
    let tool_ops: std::sync::Arc<dyn crate::ToolOps> = std::sync::Arc::new(FakeToolOps::new());
    let storage_ops: std::sync::Arc<dyn crate::StorageOps> =
        std::sync::Arc::new(FakeStorageOps::new());
    let path_context = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WirePathContext::default(),
    ));
    let daemon_config = std::sync::Arc::new(std::sync::RwLock::new(
        crate::wire::WireDaemonConfig::default(),
    ));
    let session_states = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let latest = Arc::new(Mutex::new(status("sess-1")));
    let external_ops: std::sync::Arc<dyn crate::ExternalProtocolOps> = std::sync::Arc::new(
        crate::CompositeExternalProtocolOps::new(
            std::sync::Arc::new(ChannelCommandOps::new(commands.clone())),
            session_ops.clone(),
            std::sync::Arc::new(LiveSessionObservability::new(
                session_ops.clone(),
                session_states.clone(),
                latest.clone(),
                "sess-1",
            )),
            graph_ops.clone(),
            tool_ops.clone(),
            storage_ops.clone(),
            std::sync::Arc::new(SharedSettingsOps::new(
                path_context.clone(),
                daemon_config.clone(),
                commands.clone(),
            )),
        ),
    );
    HttpState {
        commands,
        snapshots: broadcast::channel(16).0,
        latest,
        session_states,
        completer: SlashCompleter::from_commands(vec!["/help".into()]),
        events: broadcast::channel::<WireAgentEvent>(16).0,
        dag_events: broadcast::channel::<WireDagEvent>(16).0,
        job_ops: Arc::new(crate::UnavailableJobOps),
        session_ops,
        path_context,
        daemon_config,
        tool_ops,
        storage_ops,
        external_ops,
    }
}

/// Rebuild the composed external service after a test mutates one of its
/// input handles (`session_ops`, shared views).
fn rebind_external_ops(state: &mut HttpState) {
    let current = state.latest.lock().session_id.clone();
    state.external_ops = Arc::new(crate::CompositeExternalProtocolOps::new(
        Arc::new(ChannelCommandOps::new(state.commands.clone())),
        state.session_ops.clone(),
        Arc::new(LiveSessionObservability::new(
            state.session_ops.clone(),
            state.session_states.clone(),
            state.latest.clone(),
            current,
        )),
        Arc::new(crate::UnavailableGraphOps),
        state.tool_ops.clone(),
        state.storage_ops.clone(),
        Arc::new(SharedSettingsOps::new(
            state.path_context.clone(),
            state.daemon_config.clone(),
            state.commands.clone(),
        )),
    ));
}

#[derive(Default)]
struct CoverageJobOps {
    output: std::sync::Mutex<Option<WireNodeOutput>>,
}

impl JobOps for CoverageJobOps {
    fn node_output(&self, _run_id: &str, _node_id: &str) -> WireNodeOutput {
        self.output.lock().unwrap().clone().unwrap_or_default()
    }
    fn interrupt_node(&self, _run_id: &str, _node_id: &str) -> bool {
        false
    }
    fn steer_node(&self, _run_id: &str, _node_id: &str, _text: String) -> bool {
        false
    }
}

#[tokio::test]
async fn dispatch_get_node_output_handles_text_messages_and_missing_job() {
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let mut state = state_with(command_tx);
    let ops = Arc::new(CoverageJobOps::default());
    *ops.output.lock().unwrap() = Some(WireNodeOutput {
        output: Some("hello graph".into()),
        messages: Some(vec![json!({"role": "user"})]),
        ..Default::default()
    });
    state.job_ops = ops;

    let result = dispatch(
        &state,
        "get_node_output",
        Some(&json!({ "run_id": "r", "node_id": "n", "offset": 6 })),
    )
    .await
    .unwrap();
    assert_eq!(result["text"], "graph");
    assert_eq!(result["total"], 11);
    assert_eq!(result["messages"][0]["role"], "user");

    let mut state = state_with(mpsc::unbounded_channel().0);
    state.job_ops = Arc::new(CoverageJobOps::default());
    let result = dispatch(
        &state,
        "graph.get_node_output",
        Some(&json!({ "run_id": "r", "node_id": "n" })),
    )
    .await
    .unwrap();
    assert_eq!(result["text"], "");
    assert_eq!(result["total"], 0);
    assert!(result["messages"].is_null());

    let err = dispatch(
        &state,
        "get_node_output",
        Some(&json!({ "run_id": "r" })),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, -32602);
}

#[tokio::test]
async fn dispatch_send_message_accepts_explicit_session_and_set_model_channel_closed() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let state = state_with(command_tx);
    let result = dispatch(
        &state,
        "send_message",
        Some(&json!({ "text": "hi", "session_id": "other" })),
    )
    .await
    .unwrap();
    assert_eq!(result["accepted"], true);
    match command_rx.try_recv().unwrap() {
        WireCommand::Submit { session_id, .. } => assert_eq!(session_id, "other"),
        other => panic!("unexpected command: {other:?}"),
    }

    drop(command_rx);
    let result = dispatch(&state, "set_model", Some(&json!({ "model": "m" })))
        .await
        .unwrap();
    assert_eq!(result["accepted"], false);
}

#[tokio::test]
async fn dispatch_path_context_skill_dirs_and_config_none() {
    let (command_tx, mut command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let state = state_with(command_tx);

    let ctx = dispatch(&state, "session.get_path_context", None)
        .await
        .unwrap();
    assert!(ctx.get("skills_dirs").is_some());

    let result = dispatch(
        &state,
        "set_skill_dirs",
        Some(&json!({ "dirs": ["/skills/a", "/skills/b"] })),
    )
    .await
    .unwrap();
    assert_eq!(result["accepted"], true);
    assert_eq!(
        state.path_context.read().unwrap().skills_dirs,
        vec!["/skills/a", "/skills/b"]
    );
    match command_rx.recv().await.unwrap() {
        WireCommand::SetSkillDirs { dirs } => assert_eq!(dirs, vec!["/skills/a", "/skills/b"]),
        other => panic!("unexpected command: {other:?}"),
    }

    let result = dispatch(&state, "configure", None).await.unwrap();
    assert_eq!(result["accepted"], true);
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => assert!(config.model.is_none()),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_create_rename_error_and_delete_last_current() {
    let (command_tx, _command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let mut state = state_with(command_tx);
    let ops = Arc::new(FakeSessionOps::new());
    ops.add_session("only");
    state.session_ops = ops.clone();
    state.latest.lock().session_id = "only".into();
    rebind_external_ops(&mut state);

    let err = dispatch(
        &state,
        "rename_session",
        Some(&json!({ "id": "only", "name": "   " })),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, -32602);

    let deleted = dispatch(
        &state,
        "delete_session",
        Some(&json!({ "id": "only" })),
    )
    .await
    .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(state.latest.lock().session_id, "");
}

#[tokio::test]
async fn rpc_missing_id_returns_invalid_request() {
    use super::helpers::test_router;
    let router = test_router(status("sess-1"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let resp: serde_json::Value = client
        .post(format!("http://{addr}/rpc"))
        .json(&json!({ "jsonrpc": "2.0", "method": "ping" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(resp["error"]["code"], -32600);
    server.abort();
}

#[tokio::test]
async fn serve_web_serves_healthz_and_handle_aborts() {
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let state = state_with(command_tx);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = serve_web(listener, state);
    let body = reqwest::get(format!("http://{addr}/healthz"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
    handle.abort();
}

#[test]
fn open_browser_command_returns_platform_command() {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let cmd = open_browser_command("http://127.0.0.1:1");
        assert_eq!(cmd.get_program(), "xdg-open");
    }
    #[cfg(target_os = "macos")]
    {
        assert_eq!(open_browser_command("http://x").get_program(), "open");
    }
    #[cfg(target_os = "windows")]
    {
        assert_eq!(open_browser_command("http://x").get_program(), "cmd");
    }
}

struct FakeWebHost {
    endpoints: Option<TransportEndpoints>,
}

#[async_trait::async_trait(?Send)]
impl crate::host::TransportHost for FakeWebHost {
    fn transport_endpoints(&mut self) -> TransportEndpoints {
        self.endpoints.take().expect("endpoints already taken")
    }

    async fn run_transport_loop(
        self: Box<Self>,
        _mode: TransportMode,
        _endpoints: TransportEndpoints,
        server_task: tokio::task::JoinHandle<anyhow::Result<()>>,
    ) -> anyhow::Result<()> {
        server_task.abort();
        Ok(())
    }
}

#[tokio::test]
async fn run_web_driver_binds_and_aborts_server_task() {
    use crate::wire::WebOptions;

    let (command_tx, command_rx) = mpsc::unbounded_channel::<WireCommand>();
    let state = state_with(command_tx);
    let (event_tx, _) = broadcast::channel::<WireAgentEvent>(16);
    let (dag_event_tx, _) = broadcast::channel::<WireDagEvent>(16);
    let (snapshot_tx, _) = broadcast::channel::<WireStatusUpdate>(16);
    let agent_fwd = tokio::spawn(std::future::pending::<()>()).abort_handle();
    let endpoints = TransportEndpoints {
        command_tx: state.commands.clone(),
        command_rx,
        snapshot_tx: snapshot_tx.clone(),
        latest: state.latest.clone(),
        session_states: Arc::new(Mutex::new(std::collections::HashMap::new())),
        events: event_tx.clone(),
        dag_events: dag_event_tx.clone(),
        completer: SlashCompleter::from_commands(Vec::new()),
        job_ops: state.job_ops.clone(),
        graph_ops: Arc::new(crate::UnavailableGraphOps),
        session_ops: state.session_ops.clone(),
        tool_ops: state.tool_ops.clone(),
        storage_ops: state.storage_ops.clone(),
        external_ops: state.external_ops.clone(),
        path_context: state.path_context.clone(),
        daemon_config: state.daemon_config.clone(),
        session_id: "sess-1".into(),
        agent_fwd,
    };
    let host: Box<dyn crate::host::TransportHost> = Box::new(FakeWebHost {
        endpoints: Some(endpoints),
    });
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen_clone = seen.clone();
    let on_listen: Option<std::sync::Arc<dyn Fn(std::net::SocketAddr) + Send + Sync>> =
        Some(std::sync::Arc::new(move |addr| {
            *seen_clone.lock().unwrap() = Some(addr);
        }));
    let options = WebOptions {
        host: "127.0.0.1".into(),
        port: 0,
        on_listen,
    };
    run_web(host, options).await.unwrap();
    let _ = seen.lock().unwrap().take();
}
