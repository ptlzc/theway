//! `state.rs` gaps: cancel, feed routing, control-plane prompts, model specs,
//! and thinking changes.

use super::super::*;
use super::*;
use crate::turn::daemon::{SUPPORTED_APIS, SessionRuntimeState, current_model_label};

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

    // set_thinking_for_session with no persisted session -> false (no
    // runtime build is attempted for an unbuilt session).
    assert!(!host.set_thinking_for_session("missing", "high").await);
}
