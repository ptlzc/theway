//! Daemon-side forwarding `ToolOps` (issue #76): file/process operations are
//! forwarded over gRPC to the controller's `ToolService` endpoint instead of
//! being executed by the daemon itself.
//!
//! The endpoint is read at call time from the shared daemon config view
//! (`WireDaemonConfig::tool_service_addr`), so a controller can push/update it
//! after the daemon is already running.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt as _;
use std::sync::RwLock;
use theway_transport::client::GrpcClient;
use theway_transport::transport::{ToolExecStream, ToolOps};
use theway_transport::wire::{
    ToolError, WireDaemonConfig, WireToolEditRequest, WireToolEditResult, WireToolExecFrame,
    WireToolExecRequest, WireToolFindRequest, WireToolFindResult, WireToolGrepRequest,
    WireToolGrepResult, WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolWriteRequest, WireToolWriteResult,
};

/// Forwarding [`ToolOps`] that connects to the controller's tool service.
pub struct ForwardingToolOps {
    config: Arc<RwLock<WireDaemonConfig>>,
    client: tokio::sync::Mutex<Option<(String, GrpcClient)>>,
}

impl ForwardingToolOps {
    pub fn new(config: Arc<RwLock<WireDaemonConfig>>) -> Self {
        Self {
            config,
            client: tokio::sync::Mutex::new(None),
        }
    }

    async fn client(&self) -> Result<GrpcClient, ToolError> {
        let addr = {
            let config = self.config.read().unwrap();
            config
                .tool_service_addr
                .clone()
                .filter(|addr| !addr.is_empty())
        };
        let Some(addr) = addr else {
            return Err(ToolError::Other(anyhow::anyhow!(
                "daemon has no controller tool endpoint configured (tool_service_addr is unset)"
            )));
        };
        let mut guard = self.client.lock().await;
        if let Some((cached_addr, _)) = guard.as_ref() {
            if cached_addr == &addr {
                return Ok(guard.as_mut().expect("checked above").1.clone());
            }
        }
        let client = GrpcClient::connect(&addr)
            .await
            .map_err(|e| ToolError::Other(anyhow::anyhow!("connect tool service {addr}: {e}")))?;
        *guard = Some((addr.clone(), client.clone()));
        Ok(client)
    }
}

#[async_trait]
impl ToolOps for ForwardingToolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        self.client()
            .await?
            .tool_read(request)
            .await
            .map_err(ToolError::other)
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        self.client()
            .await?
            .tool_write(request)
            .await
            .map_err(ToolError::other)
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        self.client()
            .await?
            .tool_edit(request)
            .await
            .map_err(ToolError::other)
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        let mut client = self.client().await?;
        let stream = client.tool_exec(request).await.map_err(ToolError::other)?;
        let stream = stream.map(|item| match item {
            Ok(frame) => frame,
            Err(e) => WireToolExecFrame::Output {
                text: format!("tool forwarding error: {e}\n"),
            },
        });
        Ok(Box::pin(stream))
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        self.client()
            .await?
            .tool_list_dir(request)
            .await
            .map_err(ToolError::other)
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        self.client()
            .await?
            .tool_grep(request)
            .await
            .map_err(ToolError::other)
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        self.client()
            .await?
            .tool_find(request)
            .await
            .map_err(ToolError::other)
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        self.client()
            .await?
            .tool_memory_save(request)
            .await
            .map_err(ToolError::other)
    }

    async fn memory_list(
        &self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        self.client()
            .await?
            .tool_memory_list(request)
            .await
            .map_err(ToolError::other)
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        self.client()
            .await?
            .tool_memory_read(request)
            .await
            .map_err(ToolError::other)
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        self.client()
            .await?
            .tool_memory_forget(request)
            .await
            .map_err(ToolError::other)
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        self.client()
            .await?
            .tool_skill_install(request)
            .await
            .map_err(ToolError::other)
    }
}
