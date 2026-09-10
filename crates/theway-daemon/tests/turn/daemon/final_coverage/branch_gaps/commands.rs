//! `commands.rs` gaps: empty session ids, credential handling, `Configure`
//! variants, and session deletion.

use std::sync::Arc;

use theway_transport::wire::{
    WireClearCredentialRequest, WireSetCredentialRequest, WireDaemonConfig,
};

use super::*;
use crate::orchestration::SessionRuntime;
use crate::turn::daemon::{SUPPORTED_APIS, current_model_label};

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
