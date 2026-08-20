use super::super::*;
use crate::testing::{FakeSessionOps, FakeStorageOps, FakeToolOps, empty_sidebar_snapshot};
use crate::wire::{WireContextUsage, WireExtensionSnapshot};

fn extension_http_state() -> (HttpState, mpsc::UnboundedReceiver<WireCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    let latest = WireStatus {
        session_id: "headless".into(),
        model: "provider:model".into(),
        model_catalog: Vec::new(),
        cwd: "/tmp".into(),
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
        tui_max_feed_lines: None,
        extensions: WireExtensionSnapshot {
            revision: 9,
            diagnostics: vec![WireExtensionDiagnostic {
                extension_id: "headless.extension".into(),
                code: "load_failed".into(),
                severity: "error".into(),
                message: "failed safely".into(),
                session_id: None,
                event: None,
                sequence: None,
                details: serde_json::Map::new(),
                redacted_fields: vec!["secret".into()],
            }],
            ..Default::default()
        },
    };
    (
        HttpState {
            commands,
            snapshots: broadcast::channel(16).0,
            latest: Arc::new(Mutex::new(latest)),
            completer: SlashCompleter::from_commands(Vec::new()),
            events: broadcast::channel(16).0,
            dag_events: broadcast::channel(16).0,
            job_ops: Arc::new(crate::UnavailableJobOps),
            session_ops: Arc::new(FakeSessionOps::new()),
            path_context: Arc::new(RwLock::new(WirePathContext::default())),
            daemon_config: Arc::new(RwLock::new(WireDaemonConfig::default())),
            tool_ops: Arc::new(FakeToolOps::new()),
            storage_ops: Arc::new(FakeStorageOps::new()),
        },
        command_rx,
    )
}

#[tokio::test]
async fn web_headless_extension_diagnostics_and_command_need_no_tui() {
    let (state, mut command_rx) = extension_http_state();
    let snapshot = dispatch(&state, "extensions.get", None).await.unwrap();
    assert_eq!(snapshot["revision"], 9);
    assert_eq!(snapshot["diagnostics"][0]["redactedFields"][0], "secret");

    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::InvokeExtensionCommand {
                has_interactive_client,
                response,
                ..
            } => {
                assert!(!has_interactive_client);
                response
                    .send(Ok(WireExtensionCommandOutcome {
                        status: "success".into(),
                        code: None,
                        message: Some("headless".into()),
                        data: None,
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let outcome = dispatch(
        &state,
        "extensions.invoke",
        Some(&serde_json::json!({"name": "check", "arguments": {}})),
    )
    .await
    .unwrap();
    assert_eq!(outcome["status"], "success");
    assert_eq!(outcome["message"], "headless");
    responder.await.unwrap();
}
