//! Daemon-side composition of [`ExternalProtocolOps`]: one object that owns
//! command dispatch, session lifecycle, observability, graph, tool, storage,
//! and settings operations. Protocol servers hold this single object and only
//! adapt parameters/errors to gRPC, JSON-RPC, or MCP.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use theway_transport::ExternalProtocolOps;
use theway_transport::external_protocol_ops::{CommandOps, SettingsOps};
use theway_transport::feed::WireFeedBlock;
use theway_transport::session_observability::{
    ListSessionMessagesRequest, SessionMessagePage, SessionObservabilityOps,
};
use theway_transport::transport::{GraphOps, SessionOps, StorageOps, ToolExecStream, ToolOps};
use theway_transport::wire::{
    SessionSummary, ToolError, WireCollapseSessionRequest, WireCollapseSessionResponse,
    WireCommand, WireDaemonConfig, WireDagRunSnapshot, WireGraphCheckpoint,
    WireLoadCronJobsRequest, WireLoadCronJobsResult, WireLoadDagRunsRequest, WireLoadDagRunsResult,
    WireLoadTriggerRulesRequest, WireLoadTriggerRulesResult, WirePathContext, WirePromptImage,
    WireSaveCronJobsRequest, WireSaveCronJobsResult, WireSaveDagRunRequest, WireSaveDagRunResult,
    WireSaveTriggerRulesRequest, WireSaveTriggerRulesResult, WireSessionGraphNode,
    WireSessionLineage, WireSessionSnapshot, WireToolEditRequest, WireToolEditResult,
    WireToolExecRequest, WireToolFindRequest, WireToolFindResult, WireToolGrepRequest,
    WireToolGrepResult, WireToolListDirRequest, WireToolListDirResult, WireToolMemoryForgetRequest,
    WireToolMemoryForgetResult, WireToolMemoryListRequest, WireToolMemoryListResult,
    WireToolMemoryReadRequest, WireToolMemoryReadResult, WireToolMemorySaveRequest,
    WireToolMemorySaveResult, WireToolReadRequest, WireToolReadResult, WireToolSkillInstallRequest,
    WireToolSkillInstallResult, WireToolWriteRequest, WireToolWriteResult,
};
use tokio::sync::mpsc;

/// The daemon's single non-streaming external service implementation.
pub(crate) struct DaemonExternalProtocolOps {
    commands: mpsc::UnboundedSender<WireCommand>,
    session_ops: Arc<dyn SessionOps>,
    observability: Arc<dyn SessionObservabilityOps>,
    graph_ops: Arc<dyn GraphOps>,
    tool_ops: Arc<dyn ToolOps>,
    storage_ops: Arc<dyn StorageOps>,
    path_context: Arc<std::sync::RwLock<WirePathContext>>,
    daemon_config: Arc<std::sync::RwLock<WireDaemonConfig>>,
}

impl DaemonExternalProtocolOps {
    pub(crate) fn new(
        commands: mpsc::UnboundedSender<WireCommand>,
        session_ops: Arc<dyn SessionOps>,
        observability: Arc<dyn SessionObservabilityOps>,
        graph_ops: Arc<dyn GraphOps>,
        tool_ops: Arc<dyn ToolOps>,
        storage_ops: Arc<dyn StorageOps>,
        path_context: Arc<std::sync::RwLock<WirePathContext>>,
        daemon_config: Arc<std::sync::RwLock<WireDaemonConfig>>,
    ) -> Self {
        Self {
            commands,
            session_ops,
            observability,
            graph_ops,
            tool_ops,
            storage_ops,
            path_context,
            daemon_config,
        }
    }
}

#[async_trait]
impl CommandOps for DaemonExternalProtocolOps {
    async fn submit(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        Ok(self
            .commands
            .send(WireCommand::Submit {
                session_id: session_id.to_string(),
                text: text.to_string(),
                images,
                interrupt,
            })
            .is_ok())
    }

    async fn trigger_now(&self, id: &str) -> Result<bool> {
        Ok(self
            .commands
            .send(WireCommand::TriggerRuleNow { id: id.to_string() })
            .is_ok())
    }

    async fn abort(&self, session_id: &str) -> Result<bool> {
        Ok(self
            .commands
            .send(WireCommand::Abort {
                session_id: session_id.to_string(),
            })
            .is_ok())
    }

    async fn resolve_control_plane(&self, session_id: &str, approve: bool) -> Result<bool> {
        Ok(self
            .commands
            .send(WireCommand::ResolveControlPlane {
                session_id: session_id.to_string(),
                approve,
            })
            .is_ok())
    }

    async fn set_model(&self, session_id: &str, spec: &str) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .commands
            .send(WireCommand::SetModel {
                session_id: session_id.to_string(),
                spec: spec.to_string(),
                response: tx,
            })
            .is_err()
        {
            return Ok(false);
        }
        Ok(rx.await.unwrap_or(false))
    }

    async fn set_thinking(&self, session_id: &str, level: &str) -> Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .commands
            .send(WireCommand::SetThinking {
                session_id: session_id.to_string(),
                level: level.to_string(),
                response: tx,
            })
            .is_err()
        {
            return Ok(false);
        }
        Ok(rx.await.unwrap_or(false))
    }
}

#[async_trait]
impl SettingsOps for DaemonExternalProtocolOps {
    async fn get_config(&self) -> Result<WireDaemonConfig> {
        Ok(self.daemon_config.read().unwrap().clone())
    }

    async fn set_config(&self, config: &WireDaemonConfig) -> Result<bool> {
        self.commands
            .send(WireCommand::Configure {
                config: config.clone(),
            })
            .map_err(|_| anyhow!("event loop command channel closed"))?;
        Ok(true)
    }

    async fn configure(&self, config: &WireDaemonConfig) -> Result<bool> {
        self.set_config(config).await
    }

    async fn get_path_context(&self) -> Result<WirePathContext> {
        Ok(self.path_context.read().unwrap().clone())
    }

    async fn set_skill_dirs(&self, dirs: &[String]) -> Result<bool> {
        self.path_context.write().unwrap().skills_dirs = dirs.to_vec();
        self.commands
            .send(WireCommand::SetSkillDirs {
                dirs: dirs.to_vec(),
            })
            .map_err(|_| anyhow!("event loop command channel closed"))?;
        Ok(true)
    }
}

#[async_trait]
impl SessionOps for DaemonExternalProtocolOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        self.session_ops.list().await
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        self.session_ops.create(session_id, metadata).await
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        self.session_ops.update_metadata(id, metadata).await
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        self.session_ops.rename(id, name).await
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        self.session_ops.delete(id).await
    }

    async fn collapse_session(
        &self,
        request: &WireCollapseSessionRequest,
    ) -> Result<WireCollapseSessionResponse> {
        self.session_ops.collapse_session(request).await
    }

    async fn get_session_graph_node(
        &self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Option<WireSessionGraphNode>> {
        self.session_ops
            .get_session_graph_node(session_id, node_id)
            .await
    }

    async fn list_session_graph_nodes(
        &self,
        session_id: &str,
    ) -> Result<Vec<WireSessionGraphNode>> {
        self.session_ops.list_session_graph_nodes(session_id).await
    }

    async fn session_lineage(&self, session_id: &str) -> Result<WireSessionLineage> {
        self.session_ops.session_lineage(session_id).await
    }

    async fn list_session_graph_node_messages(
        &self,
        session_id: &str,
        node_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<WireFeedBlock>> {
        self.session_ops
            .list_session_graph_node_messages(session_id, node_id, offset, limit)
            .await
    }

    async fn session_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        self.session_ops.session_snapshot(session_id).await
    }
}

#[async_trait]
impl SessionObservabilityOps for DaemonExternalProtocolOps {
    async fn authoritative_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        self.observability.authoritative_snapshot(session_id).await
    }

    async fn list_session_messages(
        &self,
        request: &ListSessionMessagesRequest,
    ) -> Result<SessionMessagePage> {
        self.observability.list_session_messages(request).await
    }
}

impl GraphOps for DaemonExternalProtocolOps {
    fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        self.graph_ops.cancel_run(run_id, reason);
    }

    fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String> {
        self.graph_ops.retry(run_id, node_ids)
    }

    fn skip(&self, run_id: &str, node_id: &str) -> bool {
        self.graph_ops.skip(run_id, node_id)
    }

    fn checkpoints(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> Result<Vec<WireGraphCheckpoint>> {
        self.graph_ops.checkpoints(session_id, run_id)
    }

    fn restore(&self, session_id: &str, snapshot: &str) -> Result<String> {
        self.graph_ops.restore(session_id, snapshot)
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.graph_ops.list(session_id)
    }
}

#[async_trait]
impl ToolOps for DaemonExternalProtocolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        self.tool_ops.read_file(request).await
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        self.tool_ops.write_file(request).await
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        self.tool_ops.edit_file(request).await
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        self.tool_ops.exec_command(request).await
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        self.tool_ops.list_dir(request).await
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        self.tool_ops.grep(request).await
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        self.tool_ops.find(request).await
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        self.tool_ops.memory_save(request).await
    }

    async fn memory_list(
        &self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        self.tool_ops.memory_list(request).await
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        self.tool_ops.memory_read(request).await
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        self.tool_ops.memory_forget(request).await
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        self.tool_ops.skill_install(request).await
    }
}

#[async_trait]
impl StorageOps for DaemonExternalProtocolOps {
    async fn save_dag_run(&self, request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult> {
        self.storage_ops.save_dag_run(request).await
    }

    async fn load_dag_runs(
        &self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        self.storage_ops.load_dag_runs(request).await
    }

    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        self.storage_ops.save_trigger_rules(request).await
    }

    async fn load_trigger_rules(
        &self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        self.storage_ops.load_trigger_rules(request).await
    }

    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        self.storage_ops.save_cron_jobs(request).await
    }

    async fn load_cron_jobs(
        &self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        self.storage_ops.load_cron_jobs(request).await
    }
}

impl ExternalProtocolOps for DaemonExternalProtocolOps {}
