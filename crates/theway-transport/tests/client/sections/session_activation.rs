// ── session activation and credentials (issue #26) ─────────────────

use crate::proto::theway_grpc as proto;

fn client_activate_request(work_dir: &str) -> proto::ActivateSessionRequest {
    proto::ActivateSessionRequest {
        session_id: None,
        client_key: "client-1".into(),
        name: Some("activated".into()),
        runtime: Some(proto::SessionRuntimeContext {
            work_dir: work_dir.into(),
            provider: Some("faux".into()),
            model: Some("faux".into()),
            base_url: None,
            thinking: Some(false),
        }),
    }
}

fn client_activated_summary() -> crate::wire::SessionSummary {
    crate::wire::SessionSummary {
        session_id: "sess-activated".into(),
        name: "activated".into(),
        cwd: "/tmp/theway".into(),
        model: "faux:faux".into(),
        created_at: "2026-08-01T00:00:00Z".into(),
        last_activity_at: 0,
        graph_count: 0,
        active_graph_count: 0,
        busy: false,
        preview: None,
        metadata: std::collections::HashMap::new(),
    }
}

#[tokio::test]
async fn client_activate_session_round_trips_request_and_response() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            crate::wire::WireCommand::ActivateSession { request, response } => {
                assert_eq!(request.client_key, "client-1");
                assert_eq!(request.runtime.as_ref().unwrap().work_dir, "/tmp/theway");
                response
                    .send(Ok(crate::wire::WireActivateSessionResponse {
                        session: Some(client_activated_summary()),
                        created: true,
                    }))
                    .unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let response = client
        .activate_session(client_activate_request("/tmp/theway"))
        .await
        .unwrap();
    server.await.unwrap();

    assert!(response.created);
    let session = response.session.expect("activate response session");
    assert_eq!(session.session_id, "sess-activated");
    assert_eq!(session.name, "activated");
}

#[tokio::test]
async fn client_set_credential_round_trips_write_only_secret() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;
    let sentinel = b"client-sentinel".to_vec();
    let server_sentinel = sentinel.clone();

    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            crate::wire::WireCommand::SetCredential { request, response } => {
                assert_eq!(request.session_id, "sess-1");
                assert_eq!(request.provider, "anthropic");
                assert_eq!(request.secret, server_sentinel);
                let debug = format!("{request:?}");
                assert!(
                    !debug.contains("client-sentinel"),
                    "secret leaked into debug output: {debug}"
                );
                assert!(debug.contains("<redacted>"));
                response.send(Ok(())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let accepted = client
        .set_credential("sess-1", "anthropic", sentinel)
        .await
        .unwrap();
    server.await.unwrap();
    assert!(accepted);
}

#[tokio::test]
async fn client_clear_credential_round_trips_clear_one() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            crate::wire::WireCommand::ClearCredential { request, response } => {
                assert_eq!(request.session_id, "sess-1");
                assert_eq!(request.provider.as_deref(), Some("anthropic"));
                response.send(Ok(())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let accepted = client
        .clear_credential("sess-1", Some("anthropic"))
        .await
        .unwrap();
    server.await.unwrap();
    assert!(accepted);
}

#[tokio::test]
async fn client_clear_credential_round_trips_clear_all() {
    let (mut client, mut command_rx, _snapshot_tx) = client_and_server().await;

    let server = tokio::spawn(async move {
        match command_rx.recv().await.unwrap() {
            crate::wire::WireCommand::ClearCredential { request, response } => {
                assert_eq!(request.session_id, "sess-1");
                assert!(request.provider.is_none());
                response.send(Ok(())).unwrap();
            }
            other => panic!("unexpected command: {other:?}"),
        }
    });

    let accepted = client.clear_credential("sess-1", None).await.unwrap();
    server.await.unwrap();
    assert!(accepted);
}