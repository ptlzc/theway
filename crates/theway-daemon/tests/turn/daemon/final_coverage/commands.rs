// ── web-command routing and configure ───────────────────────────────────────────

#[tokio::test]
async fn handle_web_command_resolve_control_plane_approve_forwards_allow() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        session_id: "sess-final".into(),
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
        WireCommand::ResolveControlPlane {
            session_id: "sess-final".into(),
            approve: true,
        },
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
        r#"import { defineExtension } from "@theway-ai/plugin-sdk";
export default defineExtension((api) => {
  api.on("input", () => ({ actions: [] }));
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
    let extension_host = crate::ts_extensions::SessionPluginHost::start(
        catalog,
        crate::ts_extensions::QuickJsEnginePool::new(1),
        host.session.id.clone(),
        &host.runtime.cwd,
    )
    .await;
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
async fn submit_web_text_without_model_queues_instead_of_starting_turn() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;

    let mut turn = TurnState::default();
    host.submit_web_text("hello model-less".into(), Vec::new(), false, &mut turn)
        .await;

    assert!(
        turn.fut.is_none(),
        "a model-less session must not start an LLM turn"
    );
    assert_eq!(host.session.queue.len(), 1);
    assert!(matches!(
        host.session.queue.front(),
        Some(QueuedTurn::UserPrompt { prompt, .. }) if prompt == "hello model-less"
    ));
    let feed = host.projection.feed.plain_lines(120);
    assert!(
        feed.iter().any(|line| line.contains("no model selected")),
        "the user must be told why the message is not running: {feed:?}"
    );
}

#[tokio::test]
async fn submit_web_text_for_session_without_model_queues_in_parked_session() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    host.sessions
        .get_mut("other")
        .unwrap()
        .kernel
        .harness()
        .agent()
        .state()
        .model = None;

    host.submit_web_text_for_session("other", "hello parked".into(), Vec::new(), false)
        .await;

    let parked = host.sessions.get("other").unwrap();
    assert_eq!(parked.queue.len(), 1);
    assert!(matches!(
        parked.queue.front(),
        Some(QueuedTurn::UserPrompt { prompt, .. }) if prompt == "hello parked"
    ));
    let feed = parked.projection.feed.plain_lines(120);
    assert!(
        feed.iter().any(|line| line.contains("no model selected")),
        "the parked session must explain the queue wait: {feed:?}"
    );
}

#[tokio::test]
async fn submit_web_text_interrupt_without_running_turn_starts_turn_when_model_present() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.submit_web_text("hello interrupt".into(), Vec::new(), true, &mut turn)
        .await;

    assert!(
        turn.fut.is_some(),
        "interrupt without an in-flight turn must start the prompt immediately"
    );
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn submit_web_text_interrupt_without_running_turn_queues_when_model_missing() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;

    let mut turn = TurnState::default();
    host.submit_web_text("hello interrupt model-less".into(), Vec::new(), true, &mut turn)
        .await;

    assert!(turn.fut.is_none());
    assert_eq!(host.session.queue.len(), 1);
    assert!(matches!(
        host.session.queue.front(),
        Some(QueuedTurn::UserPrompt { prompt, .. }) if prompt == "hello interrupt model-less"
    ));
}

#[tokio::test]
async fn submit_web_text_for_session_interrupt_clears_stale_queue_and_queues_new_message() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    let parked = host.sessions.get_mut("other").unwrap();
    parked.queue.push_back(QueuedTurn::UserPrompt {
        display: "stale".into(),
        prompt: "stale prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

    host.submit_web_text_for_session("other", "hello parked interrupt".into(), Vec::new(), true)
        .await;

    let parked = host.sessions.get("other").unwrap();
    assert_eq!(parked.queue.len(), 1, "interrupt must replace the stale queued job");
    assert!(matches!(
        parked.queue.front(),
        Some(QueuedTurn::UserPrompt { prompt, .. }) if prompt == "hello parked interrupt"
    ));
}

#[tokio::test]
async fn submit_web_text_for_session_interrupt_without_model_queues_new_message() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    host.sessions
        .get_mut("other")
        .unwrap()
        .kernel
        .harness()
        .agent()
        .state()
        .model = None;

    host.submit_web_text_for_session(
        "other",
        "hello parked interrupt model-less".into(),
        Vec::new(),
        true,
    )
    .await;

    let parked = host.sessions.get("other").unwrap();
    assert_eq!(parked.queue.len(), 1);
    assert!(matches!(
        parked.queue.front(),
        Some(QueuedTurn::UserPrompt { prompt, .. }) if prompt == "hello parked interrupt model-less"
    ));
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
    // Shared with every bridged module that mutates the process-global cron
    // registry (issue #141).
    let _cron_guard = crate::triggers::CRON_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

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
        input: None,
        persisted: false,});
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "second".into(),
        prompt: "second prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

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
        input: None,
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
async fn start_next_queued_turn_holds_job_until_model_is_assigned() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "waiting".into(),
        prompt: "waiting prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

    let mut turn = TurnState::default();
    assert!(
        !host.start_next_queued_turn(&mut turn),
        "a model-less session must not consume the queued job"
    );
    assert_eq!(host.session.queue.len(), 1);
    assert!(turn.fut.is_none());
    let waiting_feed = host.projection.feed.plain_lines(120);
    assert!(
        waiting_feed
            .iter()
            .any(|line| line.contains("waiting for a model")),
        "the queue wait must be visible: {waiting_feed:?}"
    );

    host.session.kernel.harness().agent().state().model = Some(faux_model(Vec::new()));
    assert!(host.start_next_queued_turn(&mut turn));
    assert!(turn.fut.is_some(), "the held job starts once a model exists");
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn start_parked_turn_holds_job_until_model_is_assigned() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    {
        let parked = host.sessions.get_mut("other").unwrap();
        parked.kernel.harness().agent().state().model = None;
        parked.queue.push_back(QueuedTurn::UserPrompt {
            display: "parked waiting".into(),
            prompt: "parked waiting prompt".into(),
            images: Vec::new(),
        input: None,
        persisted: false,});
    }
    let mut unordered = futures::stream::FuturesUnordered::new();

    assert!(
        !host.start_parked_turn("other", &mut unordered),
        "a model-less parked session must not consume the queued job"
    );
    assert_eq!(host.sessions.get("other").unwrap().queue.len(), 1);
    assert!(unordered.is_empty());

    host.sessions
        .get_mut("other")
        .unwrap()
        .kernel
        .harness()
        .agent()
        .state()
        .model = Some(faux_model(Vec::new()));
    assert!(host.start_parked_turn("other", &mut unordered));
    assert!(host.sessions.get("other").unwrap().queue.is_empty());
    assert!(!unordered.is_empty(), "the held parked job starts");
}

#[tokio::test]
async fn set_model_command_releases_queued_job() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "held".into(),
        prompt: "held prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|model| SUPPORTED_APIS.contains(&model.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);
    let (tx, rx) = oneshot::channel();
    let mut turn = TurnState::default();
    host.handle_web_command(
        WireCommand::SetModel {
            session_id: host.session.id.clone(),
            spec,
            response: tx,
        },
        &mut turn,
    )
    .await;

    assert!(rx.await.unwrap());
    assert!(turn.fut.is_some(), "SetModel must release the held queued job");
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn configure_model_patch_releases_queued_job() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "configured".into(),
        prompt: "configured prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|model| SUPPORTED_APIS.contains(&model.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let mut patch = WireDaemonConfig::default();
    patch.provider = Some(model.provider.0.clone());
    patch.model = Some(model.id.clone());

    let mut turn = TurnState::default();
    host.handle_configure(patch, &mut turn).await;

    assert!(turn.fut.is_some(), "Configure must release the held queued job");
    assert!(host.session.queue.is_empty());
}

#[tokio::test]
async fn dispatch_web_slash_model_spec_releases_queued_job() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;
    host.enqueue_turn(QueuedTurn::UserPrompt {
        display: "slash-model".into(),
        prompt: "slash-model prompt".into(),
        images: Vec::new(),
        input: None,
        persisted: false,});

    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|model| SUPPORTED_APIS.contains(&model.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let mut turn = TurnState::default();
    host.dispatch_web_slash(
        &format!("/model {}:{}", model.provider.0, model.id),
        &mut turn,
    )
    .await;

    assert!(
        turn.fut.is_some(),
        "the /model command must release the held queued job"
    );
    assert!(host.session.queue.is_empty());
}

#[test]
fn start_triggered_turn_skips_without_model() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.session.kernel.harness().agent().state().model = None;

    let mut turn = TurnState::default();
    host.start_triggered_turn("trace12345678".into(), &mut turn);

    assert!(
        turn.fut.is_none(),
        "a model-less trigger must not start an LLM turn"
    );
    assert!(host.session.queue.is_empty());
    let feed = host.projection.feed.plain_lines(120);
    assert!(
        feed.iter().any(|line| line.contains("trigger skipped")),
        "the trigger skip must be visible: {feed:?}"
    );
}

#[tokio::test]
async fn apply_feed_update_routes_non_trigger_updates() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let session_id = host.session.id.clone();

    host.apply_feed_update(&session_id, FeedUpdate::TextDelta("hi".into()));

    // The non-trigger path feeds the console feed rather than the trigger
    // poll slot.
    assert!(host.projection.latest_trigger_poll.is_none());
}
