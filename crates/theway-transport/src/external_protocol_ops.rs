//! The protocol-neutral external service boundary: one implementation for
//! every non-streaming command, session, observability, graph, tool, storage,
//! and settings operation. gRPC / JSON-RPC / MCP adapters only parse
//! parameters, map errors, and serialize results.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::feed::WireFeedBlock;
use crate::session_observability::{
    ListSessionMessagesRequest, SessionMessagePage, SessionObservabilityOps,
};
use crate::transport::{GraphOps, SessionOps, StorageOps, ToolExecStream, ToolOps};
use crate::wire::{
    SessionSummary, ToolError, WireCollapseSessionRequest, WireCollapseSessionResponse,
    WireDaemonConfig, WireDagRunSnapshot, WireGraphCheckpoint, WireLoadCronJobsRequest,
    WireLoadCronJobsResult, WireLoadDagRunsRequest, WireLoadDagRunsResult,
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

/// Command-control domain shared by the three protocol surfaces.
#[async_trait]
pub trait CommandOps: Send + Sync {
    /// Submit a user prompt (queue or interrupt).
    async fn submit(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool>;

    /// Fire an immediate trigger rule now.
    async fn trigger_now(&self, id: &str) -> Result<bool>;

    /// Abort the in-flight turn of a session.
    async fn abort(&self, session_id: &str) -> Result<bool>;

    /// Resolve a pending control-plane approval prompt.
    async fn resolve_control_plane(&self, session_id: &str, approve: bool) -> Result<bool>;

    /// Switch the active model of a session.
    async fn set_model(&self, session_id: &str, spec: &str) -> Result<bool>;

    /// Switch the active thinking level of a session.
    async fn set_thinking(&self, session_id: &str, level: &str) -> Result<bool>;
}

/// Settings / path-context domain shared by the three protocol surfaces.
#[async_trait]
pub trait SettingsOps: Send + Sync {
    /// Current authoritative daemon configuration view.
    async fn get_config(&self) -> Result<WireDaemonConfig>;

    /// Queue a partial daemon configuration update.
    async fn set_config(&self, config: &WireDaemonConfig) -> Result<bool>;

    /// Alias of `set_config` for clients that use the `configure` verb.
    async fn configure(&self, config: &WireDaemonConfig) -> Result<bool>;

    /// Daemon path context (home / base / work dir / skill dirs).
    async fn get_path_context(&self) -> Result<WirePathContext>;

    /// Replace the extra skill directories and hot-reload skills.
    async fn set_skill_dirs(&self, dirs: &[String]) -> Result<bool>;
}

/// Combined non-streaming external service. Every operation a gRPC, JSON-RPC,
/// or MCP client can perform is dispatched through this object; protocol
/// adapters never contain business logic.
#[async_trait]
pub trait ExternalProtocolOps:
    CommandOps + SessionOps + SessionObservabilityOps + GraphOps + ToolOps + StorageOps + SettingsOps
{
}

/// Single failure message for every [`UnavailableCommandOps`] operation.
pub const COMMAND_OPS_UNAVAILABLE: &str = "command operations are unavailable";

/// Placeholder [`CommandOps`] for hosts/tests that only exercise unrelated
/// protocol surfaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableCommandOps;

#[async_trait]
impl CommandOps for UnavailableCommandOps {
    async fn submit(
        &self,
        _session_id: &str,
        _text: &str,
        _images: Vec<WirePromptImage>,
        _interrupt: bool,
    ) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }

    async fn trigger_now(&self, _id: &str) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }

    async fn abort(&self, _session_id: &str) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }

    async fn resolve_control_plane(&self, _session_id: &str, _approve: bool) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }

    async fn set_model(&self, _session_id: &str, _spec: &str) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }

    async fn set_thinking(&self, _session_id: &str, _level: &str) -> Result<bool> {
        anyhow::bail!(COMMAND_OPS_UNAVAILABLE)
    }
}

/// Single failure message for every [`UnavailableSettingsOps`] operation.
pub const SETTINGS_OPS_UNAVAILABLE: &str = "settings operations are unavailable";

/// Placeholder [`SettingsOps`] for hosts/tests that only exercise unrelated
/// protocol surfaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableSettingsOps;

#[async_trait]
impl SettingsOps for UnavailableSettingsOps {
    async fn get_config(&self) -> Result<WireDaemonConfig> {
        anyhow::bail!(SETTINGS_OPS_UNAVAILABLE)
    }

    async fn set_config(&self, _config: &WireDaemonConfig) -> Result<bool> {
        anyhow::bail!(SETTINGS_OPS_UNAVAILABLE)
    }

    async fn configure(&self, _config: &WireDaemonConfig) -> Result<bool> {
        anyhow::bail!(SETTINGS_OPS_UNAVAILABLE)
    }

    async fn get_path_context(&self) -> Result<WirePathContext> {
        anyhow::bail!(SETTINGS_OPS_UNAVAILABLE)
    }

    async fn set_skill_dirs(&self, _dirs: &[String]) -> Result<bool> {
        anyhow::bail!(SETTINGS_OPS_UNAVAILABLE)
    }
}

/// Generic composition used by tests and by hosts that already own separate
/// operation objects. The daemon normally builds its own composition; this
/// type keeps transport tests independent of the daemon crate.
pub struct CompositeExternalProtocolOps {
    commands: Arc<dyn CommandOps>,
    sessions: Arc<dyn SessionOps>,
    observability: Arc<dyn SessionObservabilityOps>,
    graph: Arc<dyn GraphOps>,
    tools: Arc<dyn ToolOps>,
    storage: Arc<dyn StorageOps>,
    settings: Arc<dyn SettingsOps>,
}

impl CompositeExternalProtocolOps {
    pub fn new(
        commands: Arc<dyn CommandOps>,
        sessions: Arc<dyn SessionOps>,
        observability: Arc<dyn SessionObservabilityOps>,
        graph: Arc<dyn GraphOps>,
        tools: Arc<dyn ToolOps>,
        storage: Arc<dyn StorageOps>,
        settings: Arc<dyn SettingsOps>,
    ) -> Self {
        Self {
            commands,
            sessions,
            observability,
            graph,
            tools,
            storage,
            settings,
        }
    }
}

#[async_trait]
impl CommandOps for CompositeExternalProtocolOps {
    async fn submit(
        &self,
        session_id: &str,
        text: &str,
        images: Vec<WirePromptImage>,
        interrupt: bool,
    ) -> Result<bool> {
        self.commands
            .submit(session_id, text, images, interrupt)
            .await
    }

    async fn trigger_now(&self, id: &str) -> Result<bool> {
        self.commands.trigger_now(id).await
    }

    async fn abort(&self, session_id: &str) -> Result<bool> {
        self.commands.abort(session_id).await
    }

    async fn resolve_control_plane(&self, session_id: &str, approve: bool) -> Result<bool> {
        self.commands
            .resolve_control_plane(session_id, approve)
            .await
    }

    async fn set_model(&self, session_id: &str, spec: &str) -> Result<bool> {
        self.commands.set_model(session_id, spec).await
    }

    async fn set_thinking(&self, session_id: &str, level: &str) -> Result<bool> {
        self.commands.set_thinking(session_id, level).await
    }
}

#[async_trait]
impl SettingsOps for CompositeExternalProtocolOps {
    async fn get_config(&self) -> Result<WireDaemonConfig> {
        self.settings.get_config().await
    }

    async fn set_config(&self, config: &WireDaemonConfig) -> Result<bool> {
        self.settings.set_config(config).await
    }

    async fn configure(&self, config: &WireDaemonConfig) -> Result<bool> {
        self.settings.configure(config).await
    }

    async fn get_path_context(&self) -> Result<WirePathContext> {
        self.settings.get_path_context().await
    }

    async fn set_skill_dirs(&self, dirs: &[String]) -> Result<bool> {
        self.settings.set_skill_dirs(dirs).await
    }
}

#[async_trait]
impl SessionOps for CompositeExternalProtocolOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        self.sessions.list().await
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        self.sessions.create(session_id, metadata).await
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        self.sessions.update_metadata(id, metadata).await
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        self.sessions.rename(id, name).await
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        self.sessions.delete(id).await
    }

    async fn collapse_session(
        &self,
        request: &WireCollapseSessionRequest,
    ) -> Result<WireCollapseSessionResponse> {
        self.sessions.collapse_session(request).await
    }

    async fn get_session_graph_node(
        &self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Option<WireSessionGraphNode>> {
        self.sessions
            .get_session_graph_node(session_id, node_id)
            .await
    }

    async fn list_session_graph_nodes(
        &self,
        session_id: &str,
    ) -> Result<Vec<WireSessionGraphNode>> {
        self.sessions.list_session_graph_nodes(session_id).await
    }

    async fn session_lineage(&self, session_id: &str) -> Result<WireSessionLineage> {
        self.sessions.session_lineage(session_id).await
    }

    async fn list_session_graph_node_messages(
        &self,
        session_id: &str,
        node_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<WireFeedBlock>> {
        self.sessions
            .list_session_graph_node_messages(session_id, node_id, offset, limit)
            .await
    }

    async fn session_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        self.sessions.session_snapshot(session_id).await
    }
}

#[async_trait]
impl SessionObservabilityOps for CompositeExternalProtocolOps {
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

impl GraphOps for CompositeExternalProtocolOps {
    fn cancel_run(&self, run_id: &str, reason: Option<&str>) {
        self.graph.cancel_run(run_id, reason);
    }

    fn retry(&self, run_id: &str, node_ids: Option<&[String]>) -> Vec<String> {
        self.graph.retry(run_id, node_ids)
    }

    fn skip(&self, run_id: &str, node_id: &str) -> bool {
        self.graph.skip(run_id, node_id)
    }

    fn checkpoints(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) -> Result<Vec<WireGraphCheckpoint>> {
        self.graph.checkpoints(session_id, run_id)
    }

    fn restore(&self, session_id: &str, snapshot: &str) -> Result<String> {
        self.graph.restore(session_id, snapshot)
    }

    fn list(&self, session_id: &str) -> Vec<WireDagRunSnapshot> {
        self.graph.list(session_id)
    }
}

#[async_trait]
impl ToolOps for CompositeExternalProtocolOps {
    async fn read_file(
        &self,
        request: &WireToolReadRequest,
    ) -> Result<WireToolReadResult, ToolError> {
        self.tools.read_file(request).await
    }

    async fn write_file(
        &self,
        request: &WireToolWriteRequest,
    ) -> Result<WireToolWriteResult, ToolError> {
        self.tools.write_file(request).await
    }

    async fn edit_file(
        &self,
        request: &WireToolEditRequest,
    ) -> Result<WireToolEditResult, ToolError> {
        self.tools.edit_file(request).await
    }

    async fn exec_command(
        &self,
        request: &WireToolExecRequest,
    ) -> Result<ToolExecStream, ToolError> {
        self.tools.exec_command(request).await
    }

    async fn list_dir(
        &self,
        request: &WireToolListDirRequest,
    ) -> Result<WireToolListDirResult, ToolError> {
        self.tools.list_dir(request).await
    }

    async fn grep(&self, request: &WireToolGrepRequest) -> Result<WireToolGrepResult, ToolError> {
        self.tools.grep(request).await
    }

    async fn find(&self, request: &WireToolFindRequest) -> Result<WireToolFindResult, ToolError> {
        self.tools.find(request).await
    }

    async fn memory_save(
        &self,
        request: &WireToolMemorySaveRequest,
    ) -> Result<WireToolMemorySaveResult, ToolError> {
        self.tools.memory_save(request).await
    }

    async fn memory_list(
        &self,
        request: &WireToolMemoryListRequest,
    ) -> Result<WireToolMemoryListResult, ToolError> {
        self.tools.memory_list(request).await
    }

    async fn memory_read(
        &self,
        request: &WireToolMemoryReadRequest,
    ) -> Result<WireToolMemoryReadResult, ToolError> {
        self.tools.memory_read(request).await
    }

    async fn memory_forget(
        &self,
        request: &WireToolMemoryForgetRequest,
    ) -> Result<WireToolMemoryForgetResult, ToolError> {
        self.tools.memory_forget(request).await
    }

    async fn skill_install(
        &self,
        request: &WireToolSkillInstallRequest,
    ) -> Result<WireToolSkillInstallResult, ToolError> {
        self.tools.skill_install(request).await
    }
}

#[async_trait]
impl StorageOps for CompositeExternalProtocolOps {
    async fn save_dag_run(&self, request: &WireSaveDagRunRequest) -> Result<WireSaveDagRunResult> {
        self.storage.save_dag_run(request).await
    }

    async fn load_dag_runs(
        &self,
        request: &WireLoadDagRunsRequest,
    ) -> Result<WireLoadDagRunsResult> {
        self.storage.load_dag_runs(request).await
    }

    async fn save_trigger_rules(
        &self,
        request: &WireSaveTriggerRulesRequest,
    ) -> Result<WireSaveTriggerRulesResult> {
        self.storage.save_trigger_rules(request).await
    }

    async fn load_trigger_rules(
        &self,
        request: &WireLoadTriggerRulesRequest,
    ) -> Result<WireLoadTriggerRulesResult> {
        self.storage.load_trigger_rules(request).await
    }

    async fn save_cron_jobs(
        &self,
        request: &WireSaveCronJobsRequest,
    ) -> Result<WireSaveCronJobsResult> {
        self.storage.save_cron_jobs(request).await
    }

    async fn load_cron_jobs(
        &self,
        request: &WireLoadCronJobsRequest,
    ) -> Result<WireLoadCronJobsResult> {
        self.storage.load_cron_jobs(request).await
    }
}

impl ExternalProtocolOps for CompositeExternalProtocolOps {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unavailable_command_ops_fails_every_operation() {
        let ops = UnavailableCommandOps;
        for result in [
            ops.submit("sess", "hi", Vec::new(), false).await,
            ops.trigger_now("rule-1").await,
            ops.abort("sess").await,
            ops.resolve_control_plane("sess", true).await,
            ops.set_model("sess", "openai:gpt").await,
            ops.set_thinking("sess", "high").await,
        ] {
            let error = result.unwrap_err();
            assert_eq!(error.to_string(), COMMAND_OPS_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn unavailable_settings_ops_fails_every_operation() {
        let ops = UnavailableSettingsOps;
        assert_eq!(
            ops.get_config().await.unwrap_err().to_string(),
            SETTINGS_OPS_UNAVAILABLE
        );
        assert_eq!(
            ops.set_config(&WireDaemonConfig::default())
                .await
                .unwrap_err()
                .to_string(),
            SETTINGS_OPS_UNAVAILABLE
        );
        assert_eq!(
            ops.configure(&WireDaemonConfig::default())
                .await
                .unwrap_err()
                .to_string(),
            SETTINGS_OPS_UNAVAILABLE
        );
        assert_eq!(
            ops.get_path_context().await.unwrap_err().to_string(),
            SETTINGS_OPS_UNAVAILABLE
        );
        assert_eq!(
            ops.set_skill_dirs(&["dir".into()])
                .await
                .unwrap_err()
                .to_string(),
            SETTINGS_OPS_UNAVAILABLE
        );
    }
}
