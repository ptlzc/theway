//! `ToolService` gRPC implementation (issue #75): forwards the file/tool
//! operation surface to the [`crate::transport::ToolOps`] handler seam. The
//! daemon kernel implements the seam against its execution environment; this
//! module only converts proto requests into wire requests (and wire results
//! back into proto responses) via the [`crate::tools`] codecs, mapping
//! [`crate::wire::ToolError`] onto tonic status codes.
//!
//! `ExecCommand` is the one streaming RPC: the handler's frame stream is
//! converted frame-by-frame into `ExecOutputFrame` messages (zero or more
//! output chunks, then the terminal exit frame).
//!
//! Issue #77: the implementation lives on the minimal [`ToolServiceState`]
//! (just the handler seam) instead of the full [`super::GrpcState`], so a
//! client/controller can serve the same `ToolService` surface WITHOUT the
//! daemon channel stack — [`serve_tool_service`] binds a tool-only tonic
//! server (ToolService + health) on a listener for exactly that use case
//! (the TUI serves its client-side executor this way).

use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use futures::{Stream, StreamExt as _};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

use super::{GrpcState, HealthServer};
use crate::proto::theway_grpc::tool_service_server::{ToolService, ToolServiceServer};
use crate::proto::theway_grpc::{
    EditFileRequest, EditFileResponse, ExecCommandRequest, ExecOutputFrame, FindRequest,
    FindResponse, GrepRequest, GrepResponse, ListDirRequest, ListDirResponse, MemoryForgetRequest,
    MemoryForgetResponse, MemoryListRequest, MemoryListResponse, MemoryReadRequest,
    MemoryReadResponse, MemorySaveRequest, MemorySaveResponse, ReadFileRequest, ReadFileResponse,
    SkillInstallRequest, SkillInstallResponse, WriteFileRequest, WriteFileResponse,
};
use crate::tools as codec;
use crate::transport::ToolOps;

/// Minimal `ToolService` server state (issue #77): only the [`ToolOps`]
/// handler seam — no command/snapshot channels. The daemon's full gRPC
/// server registers the same service through this state (see
/// [`super::serve_grpc`]); clients/controllers that serve the tool-operation
/// surface back to a daemon (the TUI's client-side executor) construct one
/// directly and hand it to [`serve_tool_service`].
#[derive(Clone)]
pub struct ToolServiceState {
    pub tool_ops: Arc<dyn ToolOps>,
}

impl ToolServiceState {
    pub fn new(tool_ops: Arc<dyn ToolOps>) -> Self {
        Self { tool_ops }
    }
}

/// Spawn a tool-only tonic server on a bound listener (issue #77): the
/// `ToolService` surface plus the standard health service — nothing from the
/// daemon channel stack. Used by the TUI to serve its client-side executor so
/// a daemon can connect back for file/process operations. The handle
/// resolves when the server exits.
pub fn serve_tool_service(
    listener: TcpListener,
    state: ToolServiceState,
) -> tokio::task::JoinHandle<Result<()>> {
    let server = tonic::transport::Server::builder()
        .add_service(ToolServiceServer::new(state))
        .add_service(HealthServer::new(super::HealthService))
        .serve_with_incoming(TcpListenerStream::new(listener));
    tokio::spawn(async move {
        server.await?;
        Ok(())
    })
}

/// Server-streaming exec frames: one item per [`ExecOutputFrame`], ending
/// with the exit frame the handler publishes.
type ExecCommandStream = Pin<Box<dyn Stream<Item = Result<ExecOutputFrame, Status>> + Send>>;

#[tonic::async_trait]
impl ToolService for ToolServiceState {
    type ExecCommandStream = ExecCommandStream;

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<ReadFileResponse>, Status> {
        let request = codec::read_file_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .read_file(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::read_file_response_to_proto(&result)))
    }

    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<WriteFileResponse>, Status> {
        let request = codec::write_file_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .write_file(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::write_file_response_to_proto(&result)))
    }

    async fn edit_file(
        &self,
        request: Request<EditFileRequest>,
    ) -> Result<Response<EditFileResponse>, Status> {
        let request = codec::edit_file_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .edit_file(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::edit_file_response_to_proto(&result)))
    }

    async fn exec_command(
        &self,
        request: Request<ExecCommandRequest>,
    ) -> Result<Response<Self::ExecCommandStream>, Status> {
        let request = codec::exec_request_from_proto(&request.into_inner());
        let frames = self
            .tool_ops
            .exec_command(&request)
            .await
            .map_err(codec::tool_status)?;
        let stream = frames.map(|frame| Ok(codec::exec_frame_to_proto(&frame)));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_dir(
        &self,
        request: Request<ListDirRequest>,
    ) -> Result<Response<ListDirResponse>, Status> {
        let request = codec::list_dir_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .list_dir(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::list_dir_response_to_proto(&result)))
    }

    async fn grep(&self, request: Request<GrepRequest>) -> Result<Response<GrepResponse>, Status> {
        let request = codec::grep_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .grep(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::grep_response_to_proto(&result)))
    }

    async fn find(&self, request: Request<FindRequest>) -> Result<Response<FindResponse>, Status> {
        let request = codec::find_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .find(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::find_response_to_proto(&result)))
    }

    async fn memory_save(
        &self,
        request: Request<MemorySaveRequest>,
    ) -> Result<Response<MemorySaveResponse>, Status> {
        let request = codec::memory_save_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .memory_save(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::memory_save_response_to_proto(&result)))
    }

    async fn memory_list(
        &self,
        request: Request<MemoryListRequest>,
    ) -> Result<Response<MemoryListResponse>, Status> {
        let request = codec::memory_list_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .memory_list(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::memory_list_response_to_proto(&result)))
    }

    async fn memory_read(
        &self,
        request: Request<MemoryReadRequest>,
    ) -> Result<Response<MemoryReadResponse>, Status> {
        let request = codec::memory_read_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .memory_read(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::memory_read_response_to_proto(&result)))
    }

    async fn memory_forget(
        &self,
        request: Request<MemoryForgetRequest>,
    ) -> Result<Response<MemoryForgetResponse>, Status> {
        let request = codec::memory_forget_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .memory_forget(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::memory_forget_response_to_proto(
            &result,
        )))
    }

    async fn skill_install(
        &self,
        request: Request<SkillInstallRequest>,
    ) -> Result<Response<SkillInstallResponse>, Status> {
        let request = codec::skill_install_request_from_proto(&request.into_inner());
        let result = self
            .tool_ops
            .skill_install(&request)
            .await
            .map_err(codec::tool_status)?;
        Ok(Response::new(codec::skill_install_response_to_proto(
            &result,
        )))
    }
}

/// Delegation so the full daemon `GrpcState` still implements `ToolService`
/// (the transport tests and the daemon's combined gRPC server use it). Each
/// method forwards to a [`ToolServiceState`] built from the shared
/// [`crate::transport::ToolOps`] handle.
#[tonic::async_trait]
impl ToolService for GrpcState {
    type ExecCommandStream = ExecCommandStream;

    async fn read_file(
        &self,
        request: Request<ReadFileRequest>,
    ) -> Result<Response<ReadFileResponse>, Status> {
        ToolService::read_file(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn write_file(
        &self,
        request: Request<WriteFileRequest>,
    ) -> Result<Response<WriteFileResponse>, Status> {
        ToolService::write_file(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn edit_file(
        &self,
        request: Request<EditFileRequest>,
    ) -> Result<Response<EditFileResponse>, Status> {
        ToolService::edit_file(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn exec_command(
        &self,
        request: Request<ExecCommandRequest>,
    ) -> Result<Response<Self::ExecCommandStream>, Status> {
        ToolService::exec_command(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn list_dir(
        &self,
        request: Request<ListDirRequest>,
    ) -> Result<Response<ListDirResponse>, Status> {
        ToolService::list_dir(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn grep(&self, request: Request<GrepRequest>) -> Result<Response<GrepResponse>, Status> {
        ToolService::grep(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn find(&self, request: Request<FindRequest>) -> Result<Response<FindResponse>, Status> {
        ToolService::find(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn memory_save(
        &self,
        request: Request<MemorySaveRequest>,
    ) -> Result<Response<MemorySaveResponse>, Status> {
        ToolService::memory_save(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn memory_list(
        &self,
        request: Request<MemoryListRequest>,
    ) -> Result<Response<MemoryListResponse>, Status> {
        ToolService::memory_list(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn memory_read(
        &self,
        request: Request<MemoryReadRequest>,
    ) -> Result<Response<MemoryReadResponse>, Status> {
        ToolService::memory_read(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn memory_forget(
        &self,
        request: Request<MemoryForgetRequest>,
    ) -> Result<Response<MemoryForgetResponse>, Status> {
        ToolService::memory_forget(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }

    async fn skill_install(
        &self,
        request: Request<SkillInstallRequest>,
    ) -> Result<Response<SkillInstallResponse>, Status> {
        ToolService::skill_install(&ToolServiceState::new(self.tool_ops.clone()), request).await
    }
}
