use super::*;

#[tokio::test]
async fn handle_web_command_activate_session_success_applies_runtime_and_replies() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");
    let (tx, rx) = oneshot::channel();
    let host = fixture.host();

    host.handle_web_command(
        WireCommand::ActivateSession {
            request: WireActivateSessionRequest {
                session_id: None,
                client_key: "client-1".into(),
                name: Some("activated".into()),
                runtime: Some(WireSessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: Some(model.provider.0.clone()),
                    model: Some(model.id.clone()),
                    base_url: None,
                    thinking: Some(false),
                }),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    let response = rx.await.unwrap().unwrap();
    assert!(response.created);
    let summary = response.session.unwrap();
    assert_eq!(host.session.id, summary.session_id);
    assert_eq!(
        current_model_label(host.session.kernel.harness()),
        format!("{}:{}", model.provider.0, model.id)
    );
    assert_eq!(
        host.session.kernel.harness().agent().state().thinking_level,
        Some(theway_core::ThinkingLevel::Off)
    );
}

#[tokio::test]
async fn handle_web_command_activate_session_is_idempotent_by_client_key() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");

    let request = || activation_request("client-1", &work, Some(&model.provider.0), Some(&model.id));
    let host = fixture.host();
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: request(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let first = rx.await.unwrap().unwrap();
    assert!(first.created);
    let first_id = first.session.unwrap().session_id;

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: request(),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let second = rx.await.unwrap().unwrap();
    assert!(!second.created, "same client-key lookup must resume, not create");
    assert_eq!(second.session.unwrap().session_id, first_id);
}

#[tokio::test]
async fn handle_web_command_activate_session_reprovisions_after_restart() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let scratch = TempDir::new().unwrap();
    let repo_dir = TempDir::new().unwrap();
    std::fs::create_dir_all(scratch.path().join("work")).unwrap();
    let work = scratch.path().join("work").canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");

    let (mut config1, _feed1, main1) =
        daemon_config(&scratch, &repo_dir, bailing_session_factory(), "sess-extra");
    install_activator(&mut config1, main1);
    let mut host1 = TurnHost::new(config1);
    let (tx, rx) = oneshot::channel();
    host1.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request(
                "client-restart",
                &work,
                Some(&model.provider.0),
                Some(&model.id),
            ),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let first = rx.await.unwrap().unwrap();
    assert!(first.created);
    let first_id = first.session.unwrap().session_id;
    drop(host1);

    let (mut config2, _feed2, main2) =
        daemon_config(&scratch, &repo_dir, bailing_session_factory(), "sess-extra");
    install_activator(&mut config2, main2);
    let mut host2 = TurnHost::new(config2);
    let (tx, rx) = oneshot::channel();
    host2.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request(
                "client-restart",
                &work,
                Some(&model.provider.0),
                Some(&model.id),
            ),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let second = rx.await.unwrap().unwrap();
    assert!(!second.created, "restart must reprovision the existing binding");
    assert_eq!(second.session.unwrap().session_id, first_id);
}

#[tokio::test]
async fn handle_web_command_activate_session_validates_request() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let host = fixture.host();

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request("   ", &work, None, None),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let err = rx.await.unwrap().unwrap_err();
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("client_key"));

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request("client-1", &work, Some("faux"), None),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let err = rx.await.unwrap().unwrap_err();
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("supplied together"));

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request("client-1", &work, Some("faux"), Some("no-such-model")),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let err = rx.await.unwrap().unwrap_err();
    assert_eq!(err.code, "invalid_argument");
    assert!(err.message.contains("model not found"));
}

#[tokio::test]
async fn handle_web_command_activate_session_enforces_exact_binding() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new_with_activator().await;
    let work = fixture._scratch.path().join("work").canonicalize().unwrap();
    let other = fixture._scratch.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    let other = other.canonicalize().unwrap();
    let model = theway_llm_provider::list_models()
        .into_iter()
        .find(|m| SUPPORTED_APIS.contains(&m.api.0.as_str()))
        .expect("a supported model should exist in the catalog");

    let host = fixture.host();
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: activation_request("client-1", &work, Some(&model.provider.0), Some(&model.id)),
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let created = rx.await.unwrap().unwrap();
    let session_id = created.session.unwrap().session_id;

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: WireActivateSessionRequest {
                session_id: Some(session_id.clone()),
                client_key: "client-1".into(),
                name: None,
                runtime: Some(WireSessionRuntimeContext {
                    work_dir: other.display().to_string(),
                    provider: Some(model.provider.0.clone()),
                    model: Some(model.id.clone()),
                    base_url: None,
                    thinking: Some(false),
                }),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let err = rx.await.unwrap().unwrap_err();
    // The session repository is scoped to the requested canonical cwd, so a
    // session created under `work` is not visible from `other`.
    assert_eq!(err.code, "not_found");
    assert!(err.message.contains("no session matches"));

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ActivateSession {
            request: WireActivateSessionRequest {
                session_id: Some(session_id),
                client_key: "client-2".into(),
                name: None,
                runtime: Some(WireSessionRuntimeContext {
                    work_dir: work.display().to_string(),
                    provider: Some(model.provider.0.clone()),
                    model: Some(model.id.clone()),
                    base_url: None,
                    thinking: Some(false),
                }),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;
    let err = rx.await.unwrap().unwrap_err();
    assert_eq!(err.code, "failed_precondition");
    assert!(err.message.contains("different client key"));
}
