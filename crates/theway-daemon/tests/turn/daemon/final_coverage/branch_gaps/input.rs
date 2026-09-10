//! `input.rs` gaps: web text submission, parked slash dispatch, and parked
//! command-outcome handling.

use async_trait::async_trait;

use super::super::*;
use crate::turn::daemon::SessionRuntimeState;

// ── input.rs gaps ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn submit_web_text_slash_with_image_bypasses_slash_dispatch() {
    let built = build_host(harness_with_input(vec![InputModality::Image]));
    let (mut host, _scratch, _repo) = built.into_parts();
    let mut turn = TurnState::default();

    host.submit_web_text(
        "/model".into(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
        &mut turn,
    )
    .await;

    assert!(turn.fut.is_some(), "slash text with images starts an image turn");
}

#[tokio::test]
async fn submit_web_text_for_session_empty_inputs() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("other"));

    host.submit_web_text_for_session("other", String::new(), Vec::new(), false)
        .await;
    host.submit_web_text_for_session(
        "other",
        String::new(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
    )
    .await;

    let parked = host.sessions.get("other").unwrap();
    assert_eq!(parked.queue.len(), 0, "non-vision parked session rejects images");
}

#[tokio::test]
async fn submit_web_text_for_session_ensure_runtime_error_and_active_id_get_mut_none() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Bailing factory -> ensure_session_runtime error path.
    host.submit_web_text_for_session("missing", "hello".into(), Vec::new(), false)
        .await;
    assert!(
        host.projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("no session runtime for missing"))
    );

    // Active id is not in the parked registry: get_mut returns None.
    let active_id = host.session.id.clone();
    host.submit_web_text_for_session(&active_id, "hello".into(), Vec::new(), false)
        .await;
}

#[tokio::test]
async fn submit_web_text_for_session_images_model_gating_and_interleave() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Non-vision parked session rejects images.
    host.sessions.insert(SessionRuntimeState::for_test("other"));
    host.submit_web_text_for_session(
        "other",
        "look".into(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
    )
    .await;
    assert!(host.sessions.get("other").unwrap().queue.is_empty());

    // Slash text with images bypasses slash dispatch (falls through), then
    // the vision parked session accepts it as a normal image prompt.
    host.sessions.insert(SessionRuntimeState::for_test("vision"));
    let vision_model = faux_model(vec![InputModality::Image]);
    host.sessions
        .get_mut("vision")
        .unwrap()
        .kernel
        .harness()
        .agent()
        .state()
        .model = Some(vision_model);
    host.submit_web_text_for_session(
        "vision",
        "/model".into(),
        vec![png_wire_image("iVBORw0KGgo=", Some("pic.png"))],
        false,
    )
    .await;
    assert_eq!(host.sessions.get("vision").unwrap().queue.len(), 1);

    // Interleave path: !interrupt && session.busy.
    host.sessions
        .get_mut("vision")
        .unwrap()
        .busy = true;
    host.submit_web_text_for_session("vision", "hello mid-turn".into(), Vec::new(), false)
        .await;
    let parked = host.sessions.get("vision").unwrap();
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("interleaved new message"))
    );
}

#[tokio::test]
async fn dispatch_web_slash_for_session_missing_session() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.dispatch_web_slash_for_session("missing", "/help").await;
}

#[tokio::test]
async fn handle_parked_command_outcome_missing_and_queued_outcomes() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Missing session no-op.
    host.handle_parked_command_outcome(
        "missing",
        "/help",
        CommandOutcome::Handled,
    );

    // A run-style outcome does not echo the command line immediately; it
    // queues the job for `start_parked_turn`.
    host.sessions.insert(SessionRuntimeState::for_test("parked-cmd"));
    host.handle_parked_command_outcome(
        "parked-cmd",
        "/definitely-not-a-daemon-command",
        CommandOutcome::RunAgentPrompt {
            prompt: "run this".into(),
            error_context: "agent failed: ",
        },
    );
    let parked = host.sessions.get("parked-cmd").unwrap();
    assert_eq!(parked.queue.len(), 1);
    assert!(matches!(
        parked.queue.front(),
        Some(QueuedTurn::AgentPrompt { .. })
    ));
}

#[tokio::test]
async fn handle_parked_command_outcome_import_activation_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.sessions.insert(SessionRuntimeState::for_test("parked-import"));
    host.handle_parked_command_outcome(
        "parked-import",
        "/import",
        CommandOutcome::SessionImportActivation {
            session_path: std::path::PathBuf::from("/tmp/imported"),
            trigger_ids: vec!["t1".into()],
            cron_ids: vec!["c1".into()],
        },
    );
    host.handle_parked_command_outcome(
        "parked-import",
        "/import",
        CommandOutcome::SessionImportActivation {
            session_path: std::path::PathBuf::from("/tmp/imported"),
            trigger_ids: Vec::new(),
            cron_ids: Vec::new(),
        },
    );
    host.handle_parked_command_outcome(
        "parked-import",
        "/import-overflow",
        CommandOutcome::SessionImportActivation {
            session_path: std::path::PathBuf::from("/tmp/imported-overflow"),
            trigger_ids: (0..6).map(|i| format!("trigger-{i}")).collect(),
            cron_ids: vec![],
        },
    );
    host.handle_parked_command_outcome(
        "parked-import",
        "/import-overflow",
        CommandOutcome::SessionImportActivation {
            session_path: std::path::PathBuf::from("/tmp/imported-overflow"),
            trigger_ids: vec!["t1".into()],
            cron_ids: (0..6).map(|i| format!("cron-{i}")).collect(),
        },
    );

    let parked = host.sessions.get("parked-import").unwrap();
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("+1 more"))
    );
}

struct EmptyTriggerImportStubCommand;

#[async_trait]
impl SlashCommand<crate::commands::DaemonCtx> for EmptyTriggerImportStubCommand {
    fn name(&self) -> &'static str {
        "empty-trigger-import"
    }
    fn description(&self) -> &'static str {
        "stub import with empty trigger ids"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, crate::commands::DaemonCtx>,
    ) -> CommandOutcome {
        CommandOutcome::SessionImportActivation {
            session_path: std::path::PathBuf::from("/tmp/empty-trigger-import"),
            trigger_ids: Vec::new(),
            cron_ids: vec!["c1".into()],
        }
    }
}

#[tokio::test]
async fn dispatch_web_slash_import_with_empty_trigger_ids() {
    let mut registry = Registry::new();
    registry.register(Arc::new(EmptyTriggerImportStubCommand));
    let built = build_host_with(
        harness_with_input(Vec::new()),
        registry,
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();

    let mut turn = TurnState::default();
    host.dispatch_web_slash("/empty-trigger-import", &mut turn).await;
    assert!(turn.fut.is_none());
}
