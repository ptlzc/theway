// ── branch-coverage gap fillers for turn/daemon ─────────────────────────────────
//
// These tests target specific LLVM branch records that the existing suites do
// not exercise. They are included last so they can reuse the helpers (and the
// `FailingAppendStorage` test double) defined in the other `final_coverage`
// files.

use std::collections::HashMap;

use futures::stream::FuturesUnordered;
use theway_contract::session::{SessionBinding, SessionRuntimeContext};
use theway_transport::wire::{
    WireActivateSessionRequest, WireClearCredentialRequest, WireSessionRuntimeContext,
    WireSetCredentialRequest, WireStatusUpdate,
};
use tokio_util::sync::CancellationToken;

use crate::mcp_loader::{McpProvisionState, ServerConfig, ServerKind};
use crate::orchestration::{SessionRuntime, SessionRuntimeBuilder};
use crate::runtime_storage::local_runtime_storage;
use crate::session_activation::SessionActivator;
use theway_core::{AgentToolError, AgentToolResult};

// ── small helpers ───────────────────────────────────────────────────────────────

struct DummyTool {
    def: theway_llm_provider::Tool,
}

#[async_trait]
impl theway_core::AgentTool for DummyTool {
    fn definition(&self) -> &theway_llm_provider::Tool {
        &self.def
    }

    fn label(&self) -> &str {
        "dummy"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancel: CancellationToken,
        _on_update: Option<theway_core::AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        Ok(AgentToolResult::default())
    }
}

fn dummy_tool() -> Arc<dyn theway_core::AgentTool> {
    Arc::new(DummyTool {
        def: theway_llm_provider::Tool {
            name: "dummy".into(),
            description: "dummy tool".into(),
            parameters: serde_json::json!({ "type": "object" }),
        },
    })
}

fn branch_gap_faux_stream() -> theway_core::StreamFn {
    std::sync::Arc::new(|_, _, _| {
        let (stream, _sender) = theway_llm_provider::AssistantMessageEventStream::new();
        stream
    })
}

#[allow(dead_code)]
fn real_session_factory() -> SessionFactory {
    Arc::new(
        |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send,
            >,
        > {
            Box::pin(async move {
                let harness = harness_with_input(Vec::new());
                Ok(SessionRuntime::for_test(id, harness))
            })
        },
    )
}

fn same_cwd_session_factory(cwd: std::path::PathBuf) -> SessionFactory {
    Arc::new(
        move |id: String| -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = anyhow::Result<SessionRuntime>> + Send,
            >,
        > {
            let cwd = cwd.clone();
            Box::pin(async move {
                let harness = harness_with_input(Vec::new());
                let mut runtime = SessionRuntime::for_test(id, harness);
                runtime.cwd = cwd.clone();
                Ok(runtime)
            })
        },
    )
}

fn register_session_binding(host: &mut TurnHost, id: &str) {
    let work = host.runtime.cwd.clone();
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    host.automation
        .services
        .session_execution
        .set(
            id,
            SessionBinding {
                client_key: "client-1".into(),
                runtime: SessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: None,
                    model: None,
                    base_url: None,
                    thinking: None,
                },
            },
        )
        .unwrap();
}

fn install_activator_after_host(host: &mut TurnHost) -> Arc<SessionRuntimeBuilder> {
    let (main_run_tx, main_run_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Keep the receiver alive for the builder's lifetime; runtime assembly may
    // send main-run triggers later.
    std::mem::forget(main_run_rx);
    let builder = Arc::new(SessionRuntimeBuilder {
        thinking: theway_core::ThinkingLevel::High,
        stream_fn: branch_gap_faux_stream(),
        dag_engine: host.automation.dag.clone(),
        subagent_registry: host.automation.subagents.clone(),
        services: host.automation.services.clone(),
        before_tool_call: None,
        control_plane_hook: None,
        control_plane_prompt_tx: None,
        after_tool_call: None,
        feed_tx: host.inputs.feed_tx.clone(),
        main_run_tx,
        debug: false,
        session_cells: Default::default(),
    });
    let activator = SessionActivator::new(
        &builder,
        local_runtime_storage(),
        host.runtime.paths.clone(),
        theway_core::ThinkingLevel::High,
        Vec::new(),
        Vec::new(),
        false,
    );
    let _ = host
        .automation
        .services
        .session_activator
        .set(Arc::new(activator));
    builder
}

async fn activate_turn_with_future(
    host: &mut TurnHost,
    work_dir: &std::path::Path,
) -> TurnState {
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let request = WireActivateSessionRequest {
        session_id: None,
        client_key: "branch-gap-client".into(),
        name: Some("branch-gap".into()),
        runtime: Some(WireSessionRuntimeContext {
            work_dir: work_dir.display().to_string(),
            provider: Some(model.provider.0.clone()),
            model: Some(model.id.clone()),
            base_url: None,
            thinking: Some(false),
        }),
    };
    let mut turn = sample_turn_with_future();
    // `handle_activate_session` awaits `apply_activation`, which aborts the
    // in-flight turn and takes its future.
    host.handle_web_command(
        WireCommand::ActivateSession {
            request,
            response: tokio::sync::oneshot::channel().0,
        },
        &mut turn,
    )
    .await;
    turn
}

fn mcp_provision_with_configs() -> McpProvisionState {
    let mut slot = McpProvisionState::default();
    slot.configs.push(ServerConfig {
        name: "slot-config".into(),
        kind: ServerKind::Stdio,
        command: Some("thewayd".into()),
        args: Vec::new(),
        endpoint: None,
        auth: None,
        request_timeout_ms: None,
        sse_idle_timeout_ms: None,
        body_cap_bytes: None,
        reconnect: None,
        inject_summary: false,
        inject_and_run: false,
    });
    slot
}

// ── queue.rs gaps ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn start_parked_turn_missing_session_and_busy_session() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let mut unordered = FuturesUnordered::new();

    assert!(
        !host.start_parked_turn("missing", &mut unordered),
        "missing parked session must report false"
    );

    host.sessions.insert(SessionRuntimeState::for_test("parked-busy"));
    host.sessions
        .get_mut("parked-busy")
        .unwrap()
        .queue
        .push_back(QueuedTurn::UserPrompt {
            display: "held".into(),
            prompt: "held".into(),
            images: Vec::new(),
        
        persisted: false,});
    host.sessions.get_mut("parked-busy").unwrap().busy = true;
    assert!(
        !host.start_parked_turn("parked-busy", &mut unordered),
        "busy parked session must report false"
    );
}

#[tokio::test]
async fn start_parked_turn_reports_remaining_and_filters_busy_or_empty_sessions() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.sessions.insert(SessionRuntimeState::for_test("parked-many"));
    {
        let session = host.sessions.get_mut("parked-many").unwrap();
        session.queue.push_back(QueuedTurn::UserPrompt {
            display: "first".into(),
            prompt: "first".into(),
            images: Vec::new(),
        
        persisted: false,});
        session.queue.push_back(QueuedTurn::UserPrompt {
            display: "second".into(),
            prompt: "second".into(),
            images: Vec::new(),
        
        persisted: false,});
    }

    let mut unordered = FuturesUnordered::new();
    assert!(host.start_parked_turn("parked-many", &mut unordered));
    let parked = host.sessions.get("parked-many").unwrap();
    assert_eq!(parked.queue.len(), 1);
    assert!(!unordered.is_empty());
    drop(unordered);

    // `start_parked_turns` must evaluate its filter for a session that does
    // not match (busy with a queued job) and a session with an empty queue.
    host.sessions.insert(SessionRuntimeState::for_test("parked-filtered"));
    host.sessions
        .get_mut("parked-filtered")
        .unwrap()
        .queue
        .push_back(QueuedTurn::UserPrompt {
            display: "filtered".into(),
            prompt: "filtered".into(),
            images: Vec::new(),
        
        persisted: false,});
    host.sessions.get_mut("parked-filtered").unwrap().busy = true;
    let mut unordered = FuturesUnordered::new();
    host.start_parked_turns(&mut unordered);
    assert!(unordered.is_empty());
}

#[tokio::test]
async fn finish_parked_turn_missing_session_aborted_and_no_usage() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let mut unordered = FuturesUnordered::new();

    // Missing session: the method returns without pushing a follow-up future.
    host.finish_parked_turn("missing", Ok(None), &mut unordered)
        .await;
    assert!(unordered.is_empty());

    // Aborted parked session: aborted branch in finish_parked_turn.
    host.sessions.insert(SessionRuntimeState::for_test("parked-aborted"));
    host.sessions.get_mut("parked-aborted").unwrap().aborted = true;
    host.finish_parked_turn("parked-aborted", Ok(None), &mut unordered)
        .await;
    let parked = host.sessions.get("parked-aborted").unwrap();
    assert!(!parked.aborted, "aborted flag is cleared after finish");
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("[aborted]"))
    );

    // Non-aborted parked session without assistant usage: the usage lookup
    // must be empty and the Ok(Some) message is echoed.
    host.sessions.insert(SessionRuntimeState::for_test("parked-no-usage"));
    host.finish_parked_turn("parked-no-usage", Ok(Some("parked done".into())), &mut unordered)
        .await;
    let parked = host.sessions.get("parked-no-usage").unwrap();
    assert!(
        parked
            .projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("parked done"))
    );
}

// ── state.rs gaps ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_session_active_and_missing() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.cancel_session(&host.session.id.clone());
    assert!(!host.session.aborted, "active cancel is a caller-handled no-op");

    host.cancel_session("missing");
}

#[tokio::test]
async fn apply_feed_update_routes_parked_and_missing_sessions() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("parked-feed"));

    let status = TriggerPollStatus {
        checked_at: "12:00:00".into(),
        trace_id: "parked-poll".into(),
        source_label: "local:dynamic".into(),
        event_label: "dynamic periodic check".into(),
        summary: "no dynamic trigger rule matched".into(),
    };

    assert!(
        host.apply_feed_update("parked-feed", FeedUpdate::TriggerPollStatus(status.clone())),
        "parked update must be applied"
    );
    assert_eq!(
        host.sessions
            .get("parked-feed")
            .unwrap()
            .projection
            .latest_trigger_poll
            .as_ref()
            .unwrap()
            .trace_id,
        "parked-poll"
    );

    assert!(
        !host.apply_feed_update("missing", FeedUpdate::TriggerPollStatus(status)),
        "missing session update must report false"
    );
}

#[tokio::test]
async fn show_control_plane_prompt_missing_session_is_a_noop() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.show_control_plane_prompt(PendingControlPlanePrompt {
        session_id: "missing".into(),
        request: ControlPlanePromptRequest {
            tool_call_id: "call-1".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install".into(),
            payload: serde_json::json!({}),
            reason: "policy".into(),
        },
        responder: oneshot::channel().0,
    });

    assert!(host.projection.control_plane_prompt.is_none());
}

#[tokio::test]
async fn resolve_control_plane_prompt_mismatched_session_restores_prompt() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let active_id = host.session.id.clone();

    // A prompt in the active projection whose session id does not match the
    // resolution target is put back into the active projection.
    let prompt = PendingControlPlanePrompt {
        session_id: "elsewhere".into(),
        request: ControlPlanePromptRequest {
            tool_call_id: "call-mismatch".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install".into(),
            payload: serde_json::json!({}),
            reason: "policy".into(),
        },
        responder: oneshot::channel().0,
    };
    host.projection.control_plane_prompt = Some(prompt);
    host.resolve_control_plane_prompt_for_session(&active_id, ControlPlanePromptDecision::Allow);
    assert!(host.projection.control_plane_prompt.is_some());

    // Same mismatch against a parked session's projection.
    let mut parked = SessionRuntimeState::for_test("parked-prompt");
    parked.projection.control_plane_prompt = Some(PendingControlPlanePrompt {
        session_id: "elsewhere".into(),
        request: ControlPlanePromptRequest {
            tool_call_id: "call-parked-mismatch".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install".into(),
            payload: serde_json::json!({}),
            reason: "policy".into(),
        },
        responder: oneshot::channel().0,
    });
    host.sessions.insert(parked);
    host.resolve_control_plane_prompt_for_session("parked-prompt", ControlPlanePromptDecision::Allow);
    assert!(host
        .sessions
        .get("parked-prompt")
        .unwrap()
        .projection
        .control_plane_prompt
        .is_some());
}

#[tokio::test]
async fn set_model_from_spec_bare_id_fallback_and_ambiguous() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Non-provider slash spec with no `:` falls through to bare-id
    // resolution and is reported as invalid.
    host.set_model_from_spec("not-a-provider/some-id").await;
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");

    // Ambiguous bare id with no base URL.
    host.set_model_from_spec("gpt-4").await;
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");

    // Bare id with a non-matching base URL falls back to the empty-base-url
    // catalog entry and applies it.
    host.runtime.config.write().unwrap().base_url = Some("http://branch-gap.invalid".into());
    let Some(model) = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| m.id == "gpt-4" && m.base_url.is_empty())
    else {
        return;
    };
    assert!(
        host.set_model_from_spec("gpt-4").await,
        "empty-base-url fallback should resolve"
    );
    assert_eq!(
        current_model_label(host.session.kernel.harness()),
        format!("{}:{}", model.provider.0, model.id)
    );
}

#[tokio::test]
async fn apply_model_reaches_activator_builder_both_states() {
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let spec = format!("{}:{}", model.provider.0, model.id);

    // Activator present and its builder is still alive.
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let _builder = install_activator_after_host(&mut host);
    assert!(host.set_model_from_spec(&spec).await);

    // Activator present but its builder has been dropped: builder() is None.
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host2, _scratch2, _repo2) = built.into_parts();
    let builder = install_activator_after_host(&mut host2);
    drop(builder);
    assert!(host2.set_model_from_spec(&spec).await);
}

#[tokio::test]
async fn apply_activation_aborts_in_flight_turn() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, scratch, _repo) = built.into_parts();
    let builder = install_activator_after_host(&mut host);
    // Keep the builder alive; activation needs it.
    let work_dir = scratch.path().join("branch-gap-work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let work_dir = work_dir.canonicalize().unwrap();

    let turn = activate_turn_with_future(&mut host, &work_dir).await;

    assert!(
        turn.fut.is_none(),
        "apply_activation must take and await the in-flight future"
    );
    assert!(!host.session.busy);
    drop(builder);
}

#[tokio::test]
async fn ensure_session_runtime_and_set_thinking_error_paths() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Active-session fast path.
    let active_id = host.session.id.clone();
    assert!(host.ensure_session_runtime(&active_id).await.is_ok());

    // Non-active session with a bailing factory -> error.
    let err = host.ensure_session_runtime("missing").await.unwrap_err();
    assert!(err.contains("build runtime for session missing"));

    // set_thinking_for_session with a bailing factory -> false.
    assert!(!host.set_thinking_for_session("missing", "high").await);
}

// ── snapshot.rs gaps ────────────────────────────────────────────────────────────

#[tokio::test]
async fn wire_snapshot_for_active_session_and_shrunk_feed() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let active_id = host.session.id.clone();
    let snapshot = host.wire_snapshot_for_session(&active_id).unwrap();
    assert_eq!(snapshot.session_id, active_id);

    // Shrink the feed without clearing block_versions: feed_dirty_start and
    // take_feed_block_patches must detect the reset.
    host.system_line("one");
    host.wire_snapshot();
    assert_eq!(host.projection.block_versions.len(), 1);
    host.projection.feed.clear();
    assert_eq!(host.feed_dirty_start(), Some(0));
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 0);
    assert!(patches.is_empty());
}

#[tokio::test]
async fn take_feed_block_patches_unchanged_dirty_and_skipped_indices() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Unchanged dirty block: fingerprint equals the stored version.
    host.system_line("one");
    host.wire_snapshot();
    host.projection.dirty_blocks.insert(0);
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 1);
    assert!(patches.is_empty());

    // Dirty index out of range: the get(index) fallback is exercised.
    host.projection.dirty_blocks.insert(100);
    let (base, patches) = host.take_feed_block_patches();
    assert_eq!(base, 1);
    assert!(patches.is_empty());

    // Two appended blocks with an empty block_versions vector: both appended
    // indices equal the growing block_versions length and are emitted.
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host2, _scratch2, _repo2) = built.into_parts();
    host2.system_line("one");
    host2.system_line("two");
    assert!(host2.projection.block_versions.is_empty());
    let (base, patches) = host2.take_feed_block_patches();
    assert_eq!(base, 0);
    assert_eq!(patches.len(), 2);
    assert_eq!(patches[0].index, 0);
    assert_eq!(patches[1].index, 1);
}

#[tokio::test]
async fn publish_snapshot_delta_paths_and_session_states() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);

    // metadata_dirty=false with session_states None: apply_to succeeds.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.runtime.session_states = None;
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to fails when the latest snapshot has no blocks and the update
    // starts at a non-zero block base.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.system_line("one");
    host.wire_update();
    host.system_line("two");
    host.runtime.session_states = None;
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to succeeds while a session_states map is present but does not
    // contain the active session id.
    let latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(HashMap::new())));
    host.publish_snapshot(&latest, &snapshots, false).await;

    // And with the active session already present in the map.
    let mut states = HashMap::new();
    states.insert(host.session.id.clone(), host.wire_snapshot());
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(states)));
    host.publish_snapshot(&latest, &snapshots, false).await;

    // apply_to fails while session_states is present: the else branch must
    // publish a full snapshot and insert it into the map.
    host.clear_feed();
    let stale_latest = Arc::new(parking_lot::Mutex::new(host.wire_snapshot()));
    host.system_line("three");
    host.wire_update();
    host.system_line("four");
    host.runtime.session_states = Some(Arc::new(parking_lot::Mutex::new(HashMap::new())));
    host.publish_snapshot(&stale_latest, &snapshots, false).await;
}

#[tokio::test]
async fn publish_parked_snapshots_without_session_states() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    host.sessions.insert(SessionRuntimeState::for_test("parked-pub"));
    host.runtime.session_states = None;

    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);
    host.publish_parked_snapshots(&snapshots);
}

#[tokio::test]
async fn publish_current_snapshot_without_runtime_channels() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // latest and snapshot_tx are both None before `transport_endpoints`.
    host.publish_current_snapshot().await;

    // latest set, snapshot_tx still None.
    host.runtime.latest = Some(Arc::new(parking_lot::Mutex::new(host.wire_snapshot())));
    host.publish_current_snapshot().await;

    // Both set: the publish path runs.
    let (snapshots, _) = tokio::sync::broadcast::channel::<WireStatusUpdate>(128);
    host.runtime.snapshot_tx = Some(snapshots);
    host.publish_current_snapshot().await;
}

#[tokio::test]
async fn wire_sidebar_snapshot_maps_trigger_rule_modes() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let once = triggers::global_registry()
        .add_rule_with_options("condition", "action", true)
        .unwrap();
    let repeat = triggers::global_registry()
        .add_rule_with_options("condition", "action", false)
        .unwrap();

    let snapshot = host.wire_snapshot();
    let rules = &snapshot.sidebar.triggers.rules;
    assert!(rules.iter().any(|rule| rule.mode == "once"));
    assert!(rules.iter().any(|rule| rule.mode == "repeat"));

    triggers::global_registry().remove_rule(&once.id).unwrap();
    triggers::global_registry().remove_rule(&repeat.id).unwrap();
}

#[tokio::test]
async fn wire_sidebar_snapshot_mcp_slot_active_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // configs non-empty: slot_active is short-circuited true.
    *host.runtime.mcp_provision.write().unwrap() = mcp_provision_with_configs();
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.servers, 0);

    // tools non-empty while configs and errors are empty.
    let mut slot = McpProvisionState::default();
    slot.tools = vec![dummy_tool()];
    slot.tool_names = vec!["dummy".into()];
    *host.runtime.mcp_provision.write().unwrap() = slot;
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.tools, 1);

    // errors non-empty while configs and tools are empty.
    let mut slot = McpProvisionState::default();
    slot.errors.push(("broken".into(), "boom".into()));
    *host.runtime.mcp_provision.write().unwrap() = slot;
    let snapshot = host.wire_snapshot();
    assert_eq!(snapshot.sidebar.mcp.errors.len(), 1);
}

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

// ── commands.rs gaps ────────────────────────────────────────────────────────────

#[tokio::test]
async fn handle_web_command_empty_session_ids_route_to_active() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_web_command(
        WireCommand::Submit {
            session_id: String::new(),
            text: "hello active".into(),
            images: Vec::new(),
            interrupt: false,
        },
        &mut TurnState::default(),
    )
    .await;

    let mut turn = sample_turn_with_future();
    host.handle_web_command(
        WireCommand::Abort {
            session_id: String::new(),
        },
        &mut turn,
    )
    .await;
    assert!(turn.aborted);

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetModel {
            session_id: String::new(),
            spec: "no-colon".into(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(!rx.await.unwrap());

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetThinking {
            session_id: String::new(),
            level: "high".into(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(rx.await.unwrap());

    // ResolveControlPlane with empty session id targets the active session.
    let (decision_tx, decision_rx) = oneshot::channel();
    host.show_control_plane_prompt(PendingControlPlanePrompt {
        session_id: host.session.id.clone(),
        request: ControlPlanePromptRequest {
            tool_call_id: "call-empty".into(),
            tool_name: "InstallSkill".into(),
            args_hash: "abc".into(),
            label: "install".into(),
            payload: serde_json::json!({}),
            reason: "policy".into(),
        },
        responder: decision_tx,
    });
    host.handle_web_command(
        WireCommand::ResolveControlPlane {
            session_id: String::new(),
            approve: true,
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(matches!(
        decision_rx.await.unwrap(),
        ControlPlanePromptDecision::Allow
    ));
}

#[tokio::test]
async fn handle_web_command_abort_unknown_session() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_web_command(
        WireCommand::Abort {
            session_id: "missing".into(),
        },
        &mut TurnState::default(),
    )
    .await;

    assert!(
        host.projection
            .feed
            .plain_lines(120)
            .iter()
            .any(|line| line.contains("abort ignored"))
    );
}

#[tokio::test]
async fn handle_set_credential_empty_session_and_provider() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let err = host
        .handle_set_credential(WireSetCredentialRequest {
            session_id: "   ".into(),
            provider: "faux".into(),
            secret: b"secret".to_vec(),
        })
        .unwrap_err();
    assert_eq!(err.code, "invalid_argument");

    let err = host
        .handle_set_credential(WireSetCredentialRequest {
            session_id: "sess-final".into(),
            provider: "   ".into(),
            secret: b"secret".to_vec(),
        })
        .unwrap_err();
    assert_eq!(err.code, "invalid_argument");
}

#[tokio::test]
async fn handle_clear_credential_empty_missing_and_provider_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    let err = host
        .handle_clear_credential(WireClearCredentialRequest {
            session_id: "   ".into(),
            provider: None,
        })
        .unwrap_err();
    assert_eq!(err.code, "invalid_argument");

    let err = host
        .handle_clear_credential(WireClearCredentialRequest {
            session_id: "missing".into(),
            provider: Some("faux".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, "not_found");

    register_session_binding(&mut host, "sess-final");

    let err = host
        .handle_clear_credential(WireClearCredentialRequest {
            session_id: "sess-final".into(),
            provider: Some("   ".into()),
        })
        .unwrap_err();
    assert_eq!(err.code, "invalid_argument");

    host.handle_clear_credential(WireClearCredentialRequest {
        session_id: "sess-final".into(),
        provider: Some("faux".into()),
    })
    .unwrap();
}

#[tokio::test]
async fn handle_configure_model_clear_and_base_url_variants() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    // Clearing provider while none is supplied.
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["provider".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Clearing provider while one IS supplied (condition is false, then the
    // supplied-together check fires because model is still None).
    host.handle_configure(
        WireDaemonConfig {
            provider: Some("anthropic".into()),
            clear_fields: vec!["provider".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Clearing model while none is supplied.
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["model".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Clearing model while one IS supplied but provider is still None.
    host.handle_configure(
        WireDaemonConfig {
            model: Some("claude-sonnet-4-5".into()),
            clear_fields: vec!["model".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // provider/model supplied separately.
    host.handle_configure(
        WireDaemonConfig {
            provider: Some("anthropic".into()),
            model: None,
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // base_url alone updates the active model's base URL.
    host.handle_configure(
        WireDaemonConfig {
            base_url: Some("http://branch-gap-base.invalid".into()),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Clearing base_url with an active model falls back to the same model
    // with an empty base URL.
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["base_url".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    // Clearing base_url with no active model hits the no-model error branch.
    host.session.kernel.harness().agent().state().model = None;
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["base_url".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
}

#[tokio::test]
async fn handle_configure_model_apply_failure_and_empty_base_url() {
    let session = Session::new(
        Arc::new(FailingAppendStorage {
            inner: Arc::new(MemorySessionStorage::new()),
        }) as Arc<dyn SessionStorage>,
    );
    let harness = harness_with_options(AgentHarnessOptions::new(faux_model(Vec::new()), session));
    let built = build_host(harness.clone());
    let (mut host, _scratch, _repo) = built.into_parts();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    host.handle_configure(
        WireDaemonConfig {
            provider: Some(model.provider.0.clone()),
            model: Some(model.id.clone()),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert_eq!(current_model_label(host.session.kernel.harness()), "faux:faux");

    // A successful model application with an empty base URL pushes the
    // `base_url` clear marker. Use a fresh host whose active faux model has an
    // empty base URL, then clear base_url: the fallback keeps the same model
    // and applies it with an empty base URL.
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host2, _scratch2, _repo2) = built.into_parts();
    host2.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["base_url".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(host2
        .session
        .kernel
        .harness()
        .agent()
        .state()
        .model
        .as_ref()
        .unwrap()
        .base_url
        .is_empty());
    assert!(host2.runtime.config.read().unwrap().base_url.is_none());
}

#[tokio::test]
async fn handle_configure_builtin_skills_and_clear() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_configure(
        WireDaemonConfig {
            builtin_skills: vec!["karpathy-guidelines".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(
        host.runtime
            .config
            .read()
            .unwrap()
            .builtin_skills
            .contains(&"karpathy-guidelines".to_string())
    );

    // Clear with an explicit non-empty list: requested stays populated.
    host.handle_configure(
        WireDaemonConfig {
            builtin_skills: vec!["karpathy-guidelines".into()],
            clear_fields: vec!["builtin_skills".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(
        host.runtime
            .config
            .read()
            .unwrap()
            .builtin_skills
            .contains(&"karpathy-guidelines".to_string())
    );

    // Clear with an empty list: requested becomes empty and the field clears.
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["builtin_skills".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(host
        .runtime
        .config
        .read()
        .unwrap()
        .builtin_skills
        .is_empty());
}

#[tokio::test]
async fn handle_configure_skills_dirs_tool_service_and_storage_clears() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();

    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["skills_dirs".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(host
        .runtime
        .config
        .read()
        .unwrap()
        .skills_dirs
        .is_empty());

    host.handle_configure(
        WireDaemonConfig {
            trigger_poll_secs: None,
            clear_fields: vec!["trigger_poll_secs".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    assert!(host
        .runtime
        .config
        .read()
        .unwrap()
        .trigger_poll_secs
        .is_none());

    host.handle_configure(
        WireDaemonConfig {
            tool_service_addr: Some("   ".into()),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    host.handle_configure(
        WireDaemonConfig {
            tool_service_addr: Some("http://tool-service".into()),
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["tool_service_addr".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;

    host.handle_configure(
        WireDaemonConfig {
            clear_fields: vec!["storage_service_addr".into()],
            ..Default::default()
        },
        &mut TurnState::default(),
    )
    .await;
}

#[tokio::test]
async fn handle_session_deleted_active_with_in_flight_turn_and_same_cwd() {
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let cwd = host.session.cwd.clone();
    std::fs::create_dir_all(&cwd).unwrap();
    let repo = host.session.repository.clone();
    repo.create_with_id(&cwd, Some("s1")).await.unwrap();
    repo.create_with_id(&cwd, Some("s2")).await.unwrap();

    // Replace the active runtime with one whose id is "s1" and whose factory
    // builds fallback runtimes with the SAME cwd as the active runtime.
    let runtime = SessionRuntime::for_test("s1", harness_with_input(Vec::new()));
    host.session = crate::turn::daemon::SessionRuntimeState::from_runtime(
        runtime,
        same_cwd_session_factory(cwd.clone()),
        repo,
        RetrySettings::default(),
        None,
        crate::turn::daemon::FeedProjectionState::new(
            host.projection.capabilities.clone(),
            host.projection.thinking_summary.clone(),
        ),
    );

    let mut turn = sample_turn_with_future();
    host.handle_web_command(
        WireCommand::SessionDeleted { id: "s1".into() },
        &mut turn,
    )
    .await;

    assert_eq!(host.session.id, "s2");
    assert!(turn.fut.is_none(), "in-flight turn was awaited");
}

// ── runtime.rs gaps ─────────────────────────────────────────────────────────────

struct HangingSlashCommand;

#[async_trait]
impl SlashCommand<crate::commands::DaemonCtx> for HangingSlashCommand {
    fn name(&self) -> &'static str {
        "hang"
    }
    fn description(&self) -> &'static str {
        "hangs until the command handler times out"
    }
    async fn run(
        &self,
        _argv: &[String],
        _ctx: &TransportCommandCtx<'_, crate::commands::DaemonCtx>,
    ) -> CommandOutcome {
        std::future::pending::<()>().await;
        CommandOutcome::Handled
    }
}

#[tokio::test]
async fn run_transport_loop_command_handler_timeout_branch() {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let mut registry = Registry::new();
    registry.register(Arc::new(HangingSlashCommand));
    let built = build_host_with(
        harness_with_input(Vec::new()),
        registry,
        bailing_session_factory(),
        "sess-final",
        None,
    );
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    endpoints
        .command_tx
        .send(WireCommand::Submit {
            session_id: "sess-final".into(),
            text: "/hang".into(),
            images: Vec::new(),
            interrupt: false,
        })
        .unwrap();

    // The command handler hangs on purpose. The loop must time it out after
    // the 30s bound and keep serving; the server task finishes one second
    // later so the loop exits cleanly.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let server_task = tokio::spawn(async move {
        let _ = shutdown_rx.await;
        anyhow::Ok(())
    });
    let driver = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(31)).await;
        let _ = shutdown_tx.send(());
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(40),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out")
    .expect("transport loop failed");

    driver.abort();
}

#[cfg(unix)]
async fn run_loop_and_send_signal_without_turn(sig: i32) {
    let _transport_loop_guard = crate::turn::daemon::TRANSPORT_LOOP_TEST_LOCK.lock().await;
    let built = build_host(harness_with_input(Vec::new()));
    let (mut host, _scratch, _repo) = built.into_parts();
    let endpoints = host.transport_endpoints();

    let server_task = tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        anyhow::Ok(())
    });
    let signal_task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = unsafe { libc::kill(std::process::id() as i32, sig) };
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        host.run_transport_loop(TransportMode::Grpc, endpoints, server_task),
    )
    .await
    .expect("transport loop timed out while waiting for signal")
    .expect("transport loop failed");

    signal_task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_ctrl_c_without_in_flight_turn() {
    run_loop_and_send_signal_without_turn(libc::SIGINT).await;
}

#[cfg(unix)]
#[tokio::test]
async fn run_transport_loop_sigterm_without_in_flight_turn() {
    run_loop_and_send_signal_without_turn(libc::SIGTERM).await;
}
