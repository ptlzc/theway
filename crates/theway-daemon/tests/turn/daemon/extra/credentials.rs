use super::*;

#[tokio::test]
async fn handle_web_command_set_credential_stores_memory_only_secret() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new().await;
    let work = fixture._scratch.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let host = fixture.host();
    host.automation
        .services
        .session_execution
        .set(
            "sess-extra",
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
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetCredential {
            request: WireSetCredentialRequest {
                session_id: "sess-extra".into(),
                provider: "faux".into(),
                secret: b"sentinel-secret".to_vec(),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    rx.await.unwrap().unwrap();
    let secret = host
        .automation
        .services
        .session_execution
        .get_credential("sess-extra", "faux")
        .unwrap();
    assert_eq!(secret.as_bytes(), b"sentinel-secret");
    assert!(format!("{:?}", WireSetCredentialRequest {
        session_id: "sess-extra".into(),
        provider: "faux".into(),
        secret: b"sentinel-secret".to_vec(),
    })
    .contains("<redacted>"));
}

#[tokio::test]
async fn handle_web_command_clear_credential_clears_all_providers_for_session() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new().await;
    let work = fixture._scratch.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let work = work.canonicalize().unwrap();
    let host = fixture.host();
    host.automation
        .services
        .session_execution
        .set(
            "sess-extra",
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
    host.automation.services.session_execution
        .set_credential("sess-extra", "faux", b"alpha".to_vec())
        .unwrap();
    host.automation.services.session_execution
        .set_credential("sess-extra", "other", b"beta".to_vec())
        .unwrap();

    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::ClearCredential {
            request: WireClearCredentialRequest {
                session_id: "sess-extra".into(),
                provider: None,
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    rx.await.unwrap().unwrap();
    assert!(host
        .automation
        .services
        .session_execution
        .get_credential("sess-extra", "faux")
        .is_none());
    assert!(host
        .automation
        .services
        .session_execution
        .get_credential("sess-extra", "other")
        .is_none());
}

#[tokio::test]
async fn handle_web_command_set_credential_unregistered_returns_not_found() {
    let _serial = crate::test_env::ENV_LOCK.lock().unwrap();
    let mut fixture = HostFixture::new().await;
    let host = fixture.host();
    let (tx, rx) = oneshot::channel();
    host.handle_web_command(
        WireCommand::SetCredential {
            request: WireSetCredentialRequest {
                session_id: "missing".into(),
                provider: "faux".into(),
                secret: b"secret".to_vec(),
            },
            response: tx,
        },
        &mut TurnState::default(),
    )
    .await;

    let err = rx.await.unwrap().unwrap_err();
    assert_eq!(err.code, "not_found");
}
