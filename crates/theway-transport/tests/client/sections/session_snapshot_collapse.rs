// ── session snapshot / collapse / session-graph client methods ────────

use std::pin::Pin;
use std::sync::Mutex;

use futures::Stream;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

use crate::proto::theway_grpc as grpc_proto;
use crate::proto::theway_grpc::session_service_server::{SessionService, SessionServiceServer};

/// Minimal in-memory `SessionService` used to test the new client methods
/// without depending on the daemon-side `GrpcState` implementation (which is
/// intentionally out of scope for this task).
#[derive(Default, Clone)]
struct MockSessionService {
    snapshot: Option<grpc_proto::SessionSnapshot>,
    message_page: Option<grpc_proto::SessionMessagePage>,
    collapse_response: Option<grpc_proto::CollapseSessionResponse>,
    graph_node: Option<grpc_proto::SessionGraphNode>,
    messages: Vec<grpc_proto::FeedBlock>,
    stream_frames: Vec<grpc_proto::SessionGraphNodeStreamFrame>,
    requests: Arc<Mutex<Vec<String>>>,
}

fn unimpl() -> Status {
    Status::unimplemented("not used by session-snapshot-collapse client tests")
}

fn proto_plain_block(text: &str) -> grpc_proto::FeedBlock {
    grpc_proto::FeedBlock {
        kind: Some(grpc_proto::feed_block::Kind::Plain(
            grpc_proto::PlainBlock {
                text: text.to_string(),
                level: "output".to_string(),
                timestamp: None,
            },
        )),
    }
}

fn sample_node(id: &str) -> grpc_proto::SessionGraphNode {
    grpc_proto::SessionGraphNode {
        id: id.to_string(),
        session_id: "sess-new".to_string(),
        r#type: grpc_proto::SessionGraphNodeType::Collapsed as i32,
        title: "Archived".to_string(),
        summary: "Old work".to_string(),
        parent_node_id: None,
        child_node_ids: Vec::new(),
        collapsed_session_id: Some("sess-old".to_string()),
        collapsed_at: Some("2026-08-01T00:00:00Z".to_string()),
        created_at: None,
        updated_at: None,
        message_count: 7,
    }
}

fn sample_snapshot(session_id: &str) -> grpc_proto::SessionSnapshot {
    grpc_proto::SessionSnapshot {
        session_id: session_id.to_string(),
        info: Some(grpc_proto::SessionInfo {
            id: session_id.to_string(),
            name: "main".to_string(),
            cwd: "/tmp/theway".to_string(),
            created_at: "2026-08-01T00:00:00Z".to_string(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: Default::default(),
            graph_count: 1,
            active_graph_count: 0,
            queued_count: 0,
            sidebar: None,
        }),
        runtime: Some(grpc_proto::SessionRuntime {
            model: Some(grpc_proto::ModelRef {
                provider: "provider".to_string(),
                model: "model".to_string(),
                base_url: None,
            }),
            thinking_level: grpc_proto::ThinkingLevel::High as i32,
            supported_thinking_levels: vec![
                grpc_proto::ThinkingLevel::Low as i32,
                grpc_proto::ThinkingLevel::High as i32,
            ],
            context_usage: None,
            session_context_usage: None,
            tui_max_feed_lines: None,
            shell_count: None,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: None,
            system_context: String::new(),
        }),
        feed: Some(grpc_proto::SessionFeed {
            blocks: vec![proto_plain_block("hello")],
            lines: vec!["hello".to_string()],
            blocks_base: 0,
            lines_base: 0,
            block_patches: Vec::new(),
        }),
        graph_state: Some(grpc_proto::SessionGraphState {
            dags: Vec::new(),
            subagents: Vec::new(),
            nodes: vec![sample_node("node-1")],
            active_node_id: Some("node-1".to_string()),
        }),
        lineage: Some(grpc_proto::SessionLineage {
            parent_session_id: Some("parent".to_string()),
            root_session_id: Some("root".to_string()),
            ancestor_session_ids: vec!["root".to_string()],
            child_session_ids: vec!["child".to_string()],
            collapsed_from_session_id: None,
            collapsed_into_session_id: Some("sess-new".to_string()),
        }),
    }
}

#[tonic::async_trait]
impl SessionService for MockSessionService {
    async fn get_snapshot(
        &self,
        request: Request<grpc_proto::SessionStateRequest>,
    ) -> Result<Response<grpc_proto::SessionSnapshot>, Status> {
        let session_id = request.into_inner().session_id;
        self.requests
            .lock()
            .unwrap()
            .push(format!("GetSnapshot({session_id})"));
        self.snapshot.clone().map(Response::new).ok_or_else(unimpl)
    }

    async fn list_session_messages(
        &self,
        request: Request<grpc_proto::ListSessionMessagesRequest>,
    ) -> Result<Response<grpc_proto::SessionMessagePage>, Status> {
        let request = request.into_inner();
        self.requests.lock().unwrap().push(format!(
            "ListSessionMessages({}, limit={}, before={:?})",
            request.session_id, request.limit, request.before_entry_id
        ));
        self.message_page.clone().map(Response::new).ok_or_else(unimpl)
    }

    async fn collapse_session(
        &self,
        request: Request<grpc_proto::CollapseSessionRequest>,
    ) -> Result<Response<grpc_proto::CollapseSessionResponse>, Status> {
        let request = request.into_inner();
        self.requests.lock().unwrap().push(format!(
            "CollapseSession({}, into={:?}, title={:?}, summary={:?})",
            request.session_id, request.into_session_id, request.title, request.summary
        ));
        self.collapse_response
            .clone()
            .map(Response::new)
            .ok_or_else(unimpl)
    }

    async fn get_session_graph_node(
        &self,
        request: Request<grpc_proto::GetSessionGraphNodeRequest>,
    ) -> Result<Response<grpc_proto::GetSessionGraphNodeResponse>, Status> {
        let request = request.into_inner();
        self.requests.lock().unwrap().push(format!(
            "GetSessionGraphNode({}, {})",
            request.session_id, request.node_id
        ));
        let node = self.graph_node.clone().ok_or_else(unimpl)?;
        Ok(Response::new(grpc_proto::GetSessionGraphNodeResponse {
            node: Some(node),
        }))
    }

    async fn list_session_graph_node_messages(
        &self,
        request: Request<grpc_proto::ListSessionGraphNodeMessagesRequest>,
    ) -> Result<Response<grpc_proto::ListSessionGraphNodeMessagesResponse>, Status> {
        let request = request.into_inner();
        self.requests.lock().unwrap().push(format!(
            "ListSessionGraphNodeMessages({}, {}, offset={}, limit={})",
            request.session_id, request.node_id, request.offset, request.limit
        ));
        Ok(Response::new(
            grpc_proto::ListSessionGraphNodeMessagesResponse {
                blocks: self.messages.clone(),
            },
        ))
    }

    type StreamSessionGraphNodeStream =
        Pin<Box<dyn Stream<Item = Result<grpc_proto::SessionGraphNodeStreamFrame, Status>> + Send>>;

    async fn stream_session_graph_node(
        &self,
        request: Request<grpc_proto::StreamSessionGraphNodeRequest>,
    ) -> Result<Response<Self::StreamSessionGraphNodeStream>, Status> {
        let request = request.into_inner();
        self.requests.lock().unwrap().push(format!(
            "StreamSessionGraphNode({}, {})",
            request.session_id, request.node_id
        ));
        let frames = self.stream_frames.clone();
        let stream = futures::stream::iter(frames.into_iter().map(Ok));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_sessions(
        &self,
        _request: Request<grpc_proto::Empty>,
    ) -> Result<Response<grpc_proto::ListSessionsResponse>, Status> {
        Err(unimpl())
    }

    async fn create_session(
        &self,
        _request: Request<grpc_proto::CreateSessionRequest>,
    ) -> Result<Response<grpc_proto::CreateSessionResponse>, Status> {
        Err(unimpl())
    }

    async fn rename_session(
        &self,
        _request: Request<grpc_proto::RenameSessionRequest>,
    ) -> Result<Response<grpc_proto::CommandResult>, Status> {
        Err(unimpl())
    }

    async fn delete_session(
        &self,
        _request: Request<grpc_proto::DeleteSessionRequest>,
    ) -> Result<Response<grpc_proto::DeleteSessionResponse>, Status> {
        Err(unimpl())
    }

    async fn update_session_metadata(
        &self,
        _request: Request<grpc_proto::UpdateSessionMetadataRequest>,
    ) -> Result<Response<grpc_proto::CommandResult>, Status> {
        Err(unimpl())
    }

    async fn get_path_context(
        &self,
        _request: Request<grpc_proto::Empty>,
    ) -> Result<Response<grpc_proto::PathContext>, Status> {
        Err(unimpl())
    }

    async fn set_skill_dirs(
        &self,
        _request: Request<grpc_proto::SetSkillDirsRequest>,
    ) -> Result<Response<grpc_proto::CommandResult>, Status> {
        Err(unimpl())
    }

    async fn activate_session(
        &self,
        _request: Request<grpc_proto::ActivateSessionRequest>,
    ) -> Result<Response<grpc_proto::ActivateSessionResponse>, Status> {
        Err(unimpl())
    }

    async fn set_credential(
        &self,
        _request: Request<grpc_proto::SetCredentialRequest>,
    ) -> Result<Response<grpc_proto::CommandResult>, Status> {
        Err(unimpl())
    }

    async fn clear_credential(
        &self,
        _request: Request<grpc_proto::ClearCredentialRequest>,
    ) -> Result<Response<grpc_proto::CommandResult>, Status> {
        Err(unimpl())
    }
}

async fn spawn_mock_session_server(service: &MockSessionService) -> GrpcClient {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tonic::transport::Server::builder()
        .add_service(SessionServiceServer::new(service.clone()))
        .serve_with_incoming(TcpListenerStream::new(listener));
    tokio::spawn(async move {
        server.await.unwrap();
    });
    GrpcClient::connect(&addr).await.unwrap()
}

#[tokio::test]
async fn client_get_snapshot_returns_nested_session_snapshot() {
    let service = MockSessionService {
        snapshot: Some(sample_snapshot("sess-1")),
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let snapshot = client.get_snapshot_for_session("sess-1").await.unwrap();

    assert_eq!(snapshot.session_id, "sess-1");
    assert_eq!(snapshot.info.as_ref().unwrap().id, "sess-1");
    assert_eq!(
        snapshot
            .runtime
            .as_ref()
            .unwrap()
            .model
            .as_ref()
            .unwrap()
            .provider,
        "provider"
    );
    assert_eq!(snapshot.feed.as_ref().unwrap().blocks.len(), 1);
    assert_eq!(snapshot.graph_state.as_ref().unwrap().nodes.len(), 1);
    assert_eq!(
        snapshot
            .lineage
            .as_ref()
            .unwrap()
            .collapsed_into_session_id
            .as_deref(),
        Some("sess-new")
    );
}

#[tokio::test]
async fn client_list_session_messages_round_trips_cursor_page() {
    let service = MockSessionService {
        message_page: Some(grpc_proto::SessionMessagePage {
            session_id: "sess-1".to_string(),
            blocks: vec![proto_plain_block("hello")],
            next_before_entry_id: Some("entry-1".to_string()),
            has_more: false,
            total: 1,
        }),
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let page = client
        .list_session_messages("sess-1", 50, None)
        .await
        .unwrap();

    assert_eq!(page.session_id, "sess-1");
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.next_before_entry_id.as_deref(), Some("entry-1"));
    assert!(!page.has_more);
    assert_eq!(page.total, 1);
}

#[tokio::test]
async fn client_collapse_session_round_trips_request_and_response() {
    let service = MockSessionService {
        collapse_response: Some(grpc_proto::CollapseSessionResponse {
            session_id: "sess-old".to_string(),
            node: Some(sample_node("node-collapsed")),
            collapsed: Some(grpc_proto::CollapsedSessionNode {
                node_id: "node-collapsed".to_string(),
                session_id: "sess-old".to_string(),
                title: "Archived".to_string(),
                summary: "Summary".to_string(),
                message_count: 7,
                collapsed_at: Some("2026-08-01T00:00:00Z".to_string()),
                collapsed_into_session_id: Some("sess-new".to_string()),
                collapsed_into_node_id: Some("node-collapsed".to_string()),
                original_session_ids: vec!["sess-old".to_string()],
            }),
        }),
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let response = client
        .collapse_session(grpc_proto::CollapseSessionRequest {
            session_id: "sess-old".to_string(),
            into_session_id: Some("sess-new".to_string()),
            title: Some("Archived".to_string()),
            summary: Some("Summary".to_string()),
        })
        .await
        .unwrap();

    assert_eq!(response.session_id, "sess-old");
    assert_eq!(response.node.as_ref().unwrap().id, "node-collapsed");
    assert_eq!(
        response
            .collapsed
            .as_ref()
            .unwrap()
            .collapsed_into_session_id
            .as_deref(),
        Some("sess-new")
    );
}

#[tokio::test]
async fn client_get_session_graph_node_round_trips() {
    let service = MockSessionService {
        graph_node: Some(sample_node("node-1")),
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let node = client
        .get_session_graph_node("sess-new", "node-1")
        .await
        .unwrap();

    assert_eq!(node.id, "node-1");
    assert_eq!(node.session_id, "sess-new");
    assert_eq!(node.title, "Archived");
}

#[tokio::test]
async fn client_list_session_graph_node_messages_page_sends_offset_limit() {
    let service = MockSessionService {
        messages: vec![proto_plain_block("first"), proto_plain_block("second")],
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let blocks = client
        .list_session_graph_node_messages_page("sess-new", "node-1", 10, 25)
        .await
        .unwrap();

    assert_eq!(blocks.len(), 2);
    let requests = service.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .any(|req| req.contains("offset=10, limit=25")),
        "missing paginated request in {requests:?}"
    );
}

#[tokio::test]
async fn client_list_session_graph_node_messages_convenience_uses_default_page() {
    let service = MockSessionService {
        messages: vec![proto_plain_block("hello")],
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let blocks = client
        .list_session_graph_node_messages("sess-new", "node-1")
        .await
        .unwrap();

    assert_eq!(blocks.len(), 1);
    let requests = service.requests.lock().unwrap();
    assert!(
        requests.iter().any(|req| req.contains("offset=0, limit=0")),
        "convenience call should use default page in {requests:?}"
    );
}

#[tokio::test]
async fn client_stream_session_graph_node_yields_node_and_block_frames() {
    let service = MockSessionService {
        stream_frames: vec![
            grpc_proto::SessionGraphNodeStreamFrame {
                payload: Some(grpc_proto::session_graph_node_stream_frame::Payload::Node(
                    sample_node("node-1"),
                )),
            },
            grpc_proto::SessionGraphNodeStreamFrame {
                payload: Some(grpc_proto::session_graph_node_stream_frame::Payload::Block(
                    proto_plain_block("streamed"),
                )),
            },
        ],
        ..Default::default()
    };
    let mut client = spawn_mock_session_server(&service).await;

    let mut stream = client
        .stream_session_graph_node("sess-new", "node-1")
        .await
        .unwrap();

    let first = stream.next().await.unwrap().unwrap();
    match first.payload {
        Some(grpc_proto::session_graph_node_stream_frame::Payload::Node(node)) => {
            assert_eq!(node.id, "node-1");
        }
        _ => panic!("first frame should be a node"),
    }

    let second = stream.next().await.unwrap().unwrap();
    match second.payload {
        Some(grpc_proto::session_graph_node_stream_frame::Payload::Block(block)) => {
            assert!(block.kind.is_some());
        }
        _ => panic!("second frame should be a block"),
    }

    assert!(stream.next().await.is_none());
}
