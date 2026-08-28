//! Minimal integration-test entry point for `cargo test --test grpc`.
//! The comprehensive gRPC suite lives in `tests/grpc/` and runs as part of
//! `cargo test --lib` via the test bridge; this shim exercises the public
//! storage-only server entry point from an integration-test crate.

use std::sync::Arc;

use theway_transport::grpc::{StorageServiceState, serve_storage_service};
use theway_transport::proto::theway_grpc::Empty;
use theway_transport::proto::theway_grpc::storage_service_client::StorageServiceClient;
use theway_transport::testing::{FakeSessionOps, FakeStorageOps};

#[tokio::test]
async fn grpc_storage_service_health_smoke() {
    let session_ops = Arc::new(FakeSessionOps::new());
    session_ops.add_session("sess-1");
    let state = StorageServiceState::new(session_ops, Arc::new(FakeStorageOps::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = serve_storage_service(listener, state);

    let mut client = StorageServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let response = client.list_sessions(Empty {}).await.unwrap().into_inner();
    assert_eq!(response.sessions.len(), 1);

    server.abort();
}