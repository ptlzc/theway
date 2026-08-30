// Integration coverage for external-protocol-service-unification: one
// `ExternalProtocolOps` object backs snapshot reads, message pagination,
// commands, and settings for the same `GrpcState`.

use crate::session_observability::{
    ListSessionMessagesRequest, SessionMessagePage, SessionObservabilityOps,
};
use crate::wire::WireSessionSnapshot;

struct ScriptedObservability {
    snapshot: WireSessionSnapshot,
    page: SessionMessagePage,
}

#[async_trait::async_trait]
impl SessionObservabilityOps for ScriptedObservability {
    async fn authoritative_snapshot(&self, session_id: &str) -> anyhow::Result<WireSessionSnapshot> {
        assert_eq!(session_id, "test-session");
        Ok(self.snapshot.clone())
    }

    async fn list_session_messages(
        &self,
        request: &ListSessionMessagesRequest,
    ) -> anyhow::Result<SessionMessagePage> {
        assert_eq!(request.session_id, "test-session");
        assert_eq!(request.before_entry_id.as_deref(), Some("entry-3"));
        Ok(self.page.clone())
    }
}

#[tokio::test]
async fn one_shared_service_backs_snapshot_page_command_and_settings() {
    let (mut state, mut command_rx, session_ops, tool_ops) = grpc_state_with_ops();
    let observability = Arc::new(ScriptedObservability {
        snapshot: WireSessionSnapshot::from(&fixture_snapshot("ready")),
        page: SessionMessagePage {
            session_id: "test-session".into(),
            blocks: vec![crate::feed::WireFeedBlock::User {
                text: "old".into(),
                timestamp: None,
            }],
            next_before_entry_id: Some("entry-2".into()),
            has_more: true,
            total: 4,
        },
    });
    state.external_ops = Arc::new(crate::CompositeExternalProtocolOps::new(
        Arc::new(ChannelCommandOps::new(state.commands.clone())),
        session_ops,
        observability,
        state.graph_ops.clone(),
        tool_ops,
        state.storage_ops.clone(),
        Arc::new(SharedSettingsOps::new(
            state.path_context.clone(),
            state.daemon_config.clone(),
            state.commands.clone(),
        )),
    ));

    // Snapshot + page both go through the same ops object.
    let snapshot = theway_grpc::session_service_server::SessionService::get_snapshot(
        &state,
        Request::new(theway_grpc::SessionStateRequest {
            session_id: "test-session".into(),
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert_eq!(snapshot.session_id, "sess-1");

    let page = theway_grpc::session_service_server::SessionService::list_session_messages(
        &state,
        Request::new(theway_grpc::ListSessionMessagesRequest {
            session_id: "test-session".into(),
            before_entry_id: Some("entry-3".into()),
            limit: 10,
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.next_before_entry_id.as_deref(), Some("entry-2"));
    assert_eq!(page.total, 4);

    // Commands and settings route through the same composite object.
    let accepted = theway_grpc::command_service_server::CommandService::send_message(
        &state,
        Request::new(SendMessageRequest {
            text: "hello".into(),
            images: Vec::new(),
            mode: MessageMode::Queue.into(),
            session_id: Some("test-session".into()),
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(accepted.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Submit {
            session_id, text, ..
        } => {
            assert_eq!(session_id, "test-session");
            assert_eq!(text, "hello");
        }
        other => panic!("unexpected command: {other:?}"),
    }

    let accepted = theway_grpc::settings_service_server::SettingsService::set_config(
        &state,
        Request::new(theway_grpc::DaemonConfig {
            model: Some("claude-y".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .into_inner();
    assert!(accepted.accepted);
    match command_rx.recv().await.unwrap() {
        WireCommand::Configure { config } => {
            assert_eq!(config.model.as_deref(), Some("claude-y"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}
