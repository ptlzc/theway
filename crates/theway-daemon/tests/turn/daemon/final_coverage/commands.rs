// ── web-command routing and configure ───────────────────────────────────────────

#[tokio::test]
async fn handle_web_command_resolve_control_plane_approve_forwards_allow() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        request: ControlPlanePromptRequest {
            tool_call_id: "call-approve".into(),
            tool_name: "WriteFile".into(),
            args_hash: "abc".into(),
            label: "write".into(),
            payload: serde_json::json!({}),
            reason: "reason".into(),
        },
        responder: decision_tx,
    });

    host.handle_web_command(
        WireCommand::ResolveControlPlane { approve: true },
        &mut TurnState::default(),
    )
    .await;

    assert!(host.projection.control_plane_prompt.is_none());
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Allow
    ));
}

#[tokio::test]
async fn handle_configure_applies_skill_dirs_and_trigger_poll() {
    static POLL_LOCK: Mutex<()> = Mutex::new(());
    let _poll_guard = POLL_LOCK.lock().unwrap();
    let previous = triggers::dynamic::dynamic_trigger_poll_interval_secs();

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut patch = WireDaemonConfig::default();
    patch.skills_dirs = vec!["/cfg-skill".into()];
    patch.trigger_poll_secs = Some(123);
    host.handle_configure(patch, &mut TurnState::default()).await;

    assert_eq!(
        host.runtime.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/cfg-skill")]
    );
    assert_eq!(triggers::dynamic::dynamic_trigger_poll_interval_secs(), 123);
    triggers::dynamic::set_dynamic_trigger_poll_interval_secs(previous);
}

#[tokio::test]
async fn handle_set_skill_dirs_maps_reload_error() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_set_skill_dirs(vec!["/new".into()], &mut TurnState::default())
        .await;

    assert_eq!(
        host.runtime.paths.current_extra_skill_dirs(),
        vec![PathBuf::from("/new")]
    );
}

#[tokio::test]
async fn handle_switch_session_reports_repo_errors() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Point the repo root at a file so `read_dir` fails.
    let file = tempfile::NamedTempFile::new().unwrap();
    host.session.repository = Arc::new(SqliteSessionRepo::new(file.path()));

    let original = host.session.id.clone();
    host.handle_switch_session("some-id".into(), &mut TurnState::default())
        .await;

    assert_eq!(host.session.id, original);
}

#[tokio::test]
async fn handle_switch_session_aborts_in_flight_turn_and_maps_switch_error() {
    let built = build_host_with(
        harness_with_input(Vec::new()),
        Registry::with_daemon_commands(),
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    std::fs::write(_repo.path().join("sess-two.db"), b"").unwrap();

    let mut turn = sample_turn_with_future();
    host.handle_switch_session("sess-two".into(), &mut turn)
        .await;

    assert!(turn.aborted);
    assert_eq!(host.session.id, "sess-final");
}

#[tokio::test]
async fn extension_protocol_projection_and_command_do_not_append_feed_lines() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let package = host
        .runtime
        .paths
        .base
        .join("extensions")
        .join("quiet-extension");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("theway-extension.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "id": "quiet-extension",
            "version": "1.0.0",
            "abi": 2,
            "entry": "index.js",
            "priority": 0,
            "scope": "session",
            "stateSchema": 1,
            "permissions": ["commands.register", "client.contribute"]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        package.join("index.js"),
        r#"import { defineExtension } from "theway";
export default defineExtension((api) => {
  api.on("input", () => ({ abiMajor: 2, actions: [] }));
  api.registerCommand({
    name: "quiet-check", label: "Quiet", description: "Quiet protocol check",
    argumentSchema: { type: "object" },
  }, async () => ({ status: "success", message: "quiet" }));
  api.contribute({
    contributionId: "quiet-status", extensionId: "quiet-extension", scope: "session",
    contribution: { kind: "status_item", label: "Quiet", value: "ready" },
  });
});"#,
    )
    .unwrap();
    let catalog = crate::ts_extensions::PackageCatalog::discover(
        &host.runtime.cwd,
        &host.runtime.paths.base,
    );
    let extension_host = Arc::new(
        crate::ts_extensions::SessionPluginHost::start(
            catalog,
            crate::ts_extensions::QuickJsEnginePool::new(1),
            host.session.id.clone(),
            &host.runtime.cwd,
        )
        .await,
    );
    host.session
        .kernel
        .set_extension_host(Some(extension_host.clone()));

    let before = host.wire_snapshot().feed_blocks;
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.extensions.commands[0].name, "quiet-check");
    assert_eq!(snapshot.extensions.contributions[0].kind, "status_item");
    let _ = extension_host
        .invoke(
            theway_contract::extension::ExtensionLifecycleEvent::Input,
            serde_json::json!({}),
        )
        .await;

    let (response, outcome) = oneshot::channel();
    host.handle_web_command(
        WireCommand::InvokeExtensionCommand {
            name: "quiet-check".into(),
            arguments: serde_json::json!({}),
            has_interactive_client: false,
            response,
        },
        &mut TurnState::default(),
    )
    .await;
    assert_eq!(outcome.await.unwrap().unwrap().status, "success");
    let after = host.wire_snapshot().feed_blocks;
    assert_eq!(after, before, "routine hooks/status/commands stay off the feed");
    assert!(!format!("{after:?}").contains("ai ▸"));
    extension_host.shutdown().await;
}

// ── submit / dispatch branches ──────────────────────────────────────────────────

#[tokio::test]
async fn submit_web_text_maps_image_load_error() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.submit_web_text(
        "look".into(),
        vec![png_wire_image("not base64!!!", None)],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_none());
}

#[tokio::test]
async fn submit_web_text_empty_text_with_image_starts_vision_turn() {
    let built = build_host(harness_with_input(vec![InputModality::Image]));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.submit_web_text(
        String::new(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some());
}

#[tokio::test]
async fn dispatch_web_slash_queues_template_and_compaction_when_busy() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = sample_turn_with_future();
    host.dispatch_web_slash("/template tpl k=v", &mut turn)
        .await;
    host.dispatch_web_slash("/compact", &mut turn).await;

    assert_eq!(host.session.queue.len(), 2);
    assert!(matches!(
        &host.session.queue.front(),
        Some(QueuedTurn::PromptTemplate { .. })
    ));
    assert!(matches!(
        &host.session.queue.back(),
        Some(QueuedTurn::Compaction { .. })
    ));
}

// ── sidebar rows and queued-turn variants ───────────────────────────────────────

#[tokio::test]
async fn wire_sidebar_snapshot_maps_cron_jobs() {
    static CRON_LOCK: Mutex<()> = Mutex::new(());
    let _cron_guard = CRON_LOCK.lock().unwrap();

    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let job = triggers::global_cron_registry()
        .add_job("*/5 * * * *", "echo tick")
        .unwrap();

    let snapshot = host.wire_snapshot();
    assert!(snapshot.sidebar.cron.total >= 1);
    assert!(snapshot.sidebar.cron.enabled >= 1);

    triggers::global_cron_registry().remove_job(&job.id).unwrap();
}

#[tokio::test]
async fn wire_sidebar_snapshot_maps_skills() {
    let mut options = AgentHarnessOptions::new(faux_model(Vec::new()), memory_session());
    options.skills = vec![Skill {
        name: "final-skill".into(),
        description: "skill from final coverage".into(),
        file_path: "/skills/final/SKILL.md".into(),
        content: "body".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }];
    let built = build_host(harness_with_options(options));
    let (mut host, _scratch, _repo) = built.into_parts();

    let snapshot = host.wire_snapshot();

    assert_eq!(snapshot.sidebar.skills.total, 1);
    assert_eq!(snapshot.sidebar.skills.items[0].name, "final-skill");
}

#[tokio::test]
async fn start_next_queued_turn_reports_remaining_count() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "first".into(),
        prompt: "first prompt".into(),
        images: Vec::new(),
    });
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "second".into(),
        prompt: "second prompt".into(),
        images: Vec::new(),
    });

    let mut turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(host.session.queue.len(), 1);
    assert!(turn.fut.is_some());
}

#[tokio::test]
async fn start_next_queued_turn_handles_all_job_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.enqueue_turn(QueuedTurn::AgentPrompt {
        display: "agent".into(),
        prompt: "agent prompt".into(),
        error_context: "agent failed: ",
    });
    host.enqueue_turn(QueuedTurn::PromptTemplate {
        display: "template".into(),
        name: "tpl".into(),
        vars: serde_json::Map::new(),
    });
    host.enqueue_turn(QueuedTurn::Compaction {
        display: "compaction".into(),
        custom: None,
    });

    let mut turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "agent failed: ");

    turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "template run failed: ");

    turn = TurnState::default();
    assert!(host.start_next_queued_turn(&mut turn));
    assert_eq!(turn.prefix, "compaction failed: ");
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn apply_feed_update_routes_non_trigger_updates() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.apply_feed_update(FeedUpdate::TextDelta("hi".into()));

    // The non-trigger path feeds the console feed rather than the trigger
    // poll slot.
    assert!(host.projection.latest_trigger_poll.is_none());
}
