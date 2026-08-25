use super::*;
use crate::proto::theway_grpc::extension_service_server::ExtensionService;
use crate::proto::theway_grpc::{
    DecideExtensionTrustRequest, InvokeExtensionCommandRequest, ReloadExtensionsRequest,
};

#[tokio::test]
async fn headless_extension_service_reads_diagnostics_and_invokes_command() {
    let (state, mut command_rx) = grpc_state();
    state.latest.lock().extensions = crate::wire::WireExtensionSnapshot {
        revision: 4,
        diagnostics: vec![crate::wire::WireExtensionDiagnostic {
            extension_id: "example.extension".into(),
            code: "permission_denied".into(),
            severity: "warning".into(),
            message: "permission denied".into(),
            session_id: Some("sess-1".into()),
            event: None,
            sequence: Some(8),
            details: serde_json::Map::new(),
            redacted_fields: vec!["token".into()],
        }],
        ..Default::default()
    };
    let snapshot = state
        .get_extensions(Request::new(Empty {}))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(snapshot.revision, 4);
    assert_eq!(snapshot.diagnostics[0].redacted_fields, vec!["token"]);

    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::InvokeExtensionCommand {
                name,
                arguments,
                has_interactive_client,
                response,
            } => {
                assert_eq!(name, "headless-check");
                assert_eq!(arguments, serde_json::json!({"value": 1}));
                assert!(!has_interactive_client);
                response
                    .send(Ok(crate::wire::WireExtensionCommandOutcome {
                        status: "success".into(),
                        code: None,
                        message: Some("done".into()),
                        data: Some(serde_json::json!({"ok": true})),
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let outcome = state
        .invoke_command(Request::new(InvokeExtensionCommandRequest {
            name: "headless-check".into(),
            arguments_json: r#"{"value":1}"#.into(),
            has_interactive_client: false,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(outcome.status, "success");
    assert_eq!(outcome.message.as_deref(), Some("done"));
    responder.await.unwrap();
}

#[tokio::test]
async fn headless_extension_service_forwards_reload_and_trust() {
    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::ReloadExtensions {
                cancel_active,
                response,
            } => {
                assert!(cancel_active);
                response
                    .send(Ok(crate::wire::WireExtensionReloadResult {
                        status: "pending".into(),
                        revision: 2,
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
        match command_rx.recv().await.unwrap() {
            WireCommand::DecideExtensionTrust { request, response } => {
                assert_eq!(request.subject, "package");
                assert_eq!(request.extension_id.as_deref(), Some("example.extension"));
                response
                    .send(Ok(crate::wire::WireExtensionTrustResult {
                        accepted: true,
                        reload: crate::wire::WireExtensionReloadResult {
                            status: "applied".into(),
                            revision: 3,
                        },
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let reload = state
        .reload(Request::new(ReloadExtensionsRequest {
            cancel_active: true,
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(reload.status, "pending");
    let trust = state
        .decide_trust(Request::new(DecideExtensionTrustRequest {
            subject: "package".into(),
            extension_id: Some("example.extension".into()),
            decision: "trusted".into(),
            granted_permissions: vec!["client.contribute".into()],
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(trust.accepted);
    assert_eq!(trust.reload.unwrap().revision, 3);
    responder.await.unwrap();
}

#[tokio::test]
async fn extension_service_rejects_bad_arguments_json() {
    let (state, _command_rx) = grpc_state();
    let err = state
        .invoke_command(Request::new(InvokeExtensionCommandRequest {
            name: "x".into(),
            arguments_json: "not-json".into(),
            has_interactive_client: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
}

#[tokio::test]
async fn extension_service_maps_closed_channels_and_failed_replies() {
    let (state, command_rx) = grpc_state();
    drop(command_rx);

    let err = state
        .invoke_command(Request::new(InvokeExtensionCommandRequest {
            name: "x".into(),
            arguments_json: "{}".into(),
            has_interactive_client: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);

    let err = state
        .reload(Request::new(ReloadExtensionsRequest {
            cancel_active: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);

    let err = state
        .decide_trust(Request::new(DecideExtensionTrustRequest {
            subject: "pkg".into(),
            extension_id: None,
            decision: "trust".into(),
            granted_permissions: vec![],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);
}

#[tokio::test]
async fn extension_service_maps_failed_preconditions() {
    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::InvokeExtensionCommand { response, .. } => {
                response.send(Err("invoke failed".into())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .invoke_command(Request::new(InvokeExtensionCommandRequest {
            name: "x".into(),
            arguments_json: "{}".into(),
            has_interactive_client: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    responder.await.unwrap();

    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::ReloadExtensions { response, .. } => {
                response.send(Err("reload failed".into())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .reload(Request::new(ReloadExtensionsRequest {
            cancel_active: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    responder.await.unwrap();

    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::DecideExtensionTrust { response, .. } => {
                response.send(Err("trust failed".into())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .decide_trust(Request::new(DecideExtensionTrustRequest {
            subject: "pkg".into(),
            extension_id: None,
            decision: "trust".into(),
            granted_permissions: vec![],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    responder.await.unwrap();
}

#[tokio::test]
async fn extension_service_maps_dropped_replies() {
    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::InvokeExtensionCommand { response, .. } => drop(response),
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .invoke_command(Request::new(InvokeExtensionCommandRequest {
            name: "x".into(),
            arguments_json: "{}".into(),
            has_interactive_client: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);
    responder.await.unwrap();
}

#[tokio::test]
async fn extension_service_maps_dropped_reload_and_trust_replies() {
    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::ReloadExtensions { response, .. } => drop(response),
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .reload(Request::new(ReloadExtensionsRequest {
            cancel_active: false,
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);
    responder.await.unwrap();

    let (state, mut command_rx) = grpc_state();
    let responder = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            WireCommand::DecideExtensionTrust { response, .. } => drop(response),
            other => panic!("unexpected command: {other:?}"),
        }
    });
    let err = state
        .decide_trust(Request::new(DecideExtensionTrustRequest {
            subject: "pkg".into(),
            extension_id: None,
            decision: "trust".into(),
            granted_permissions: vec![],
        }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unavailable);
    responder.await.unwrap();
}
