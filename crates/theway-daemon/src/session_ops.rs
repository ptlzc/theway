//! Session resources (session-resource-model) — the app-side surface the transport
//! servers program against for the session lifecycle RPCs / HTTP routes.
//!
//! * [`SessionFactory`] — builds a fresh, fully-wired session runtime for any session id
//!   (resume semantics, the in-process version of CLI `--resume-id`). Built by the daemon
//!   orchestration layer (`orchestration::SessionRuntimeBuilder`) and carried by the transport
//!   host (`turn::daemon::TurnHost`).
//! * [`SessionOps`] — sync query/mutation ops that do NOT need the event loop
//!   (list / create / rename / delete). Sessions are addressed explicitly by id; there is
//!   no daemon-side session-switch operation.
//!
//! The daemon's transport host holds an `Arc<dyn SessionOps>` (via
//! [`theway_transport::TransportEndpoints`]) and never touches the session repo
//! directly, keeping the "transport programs only against the kernel's public surface" boundary.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use theway_contract::session::{SessionReader, SessionStore, StoredSessionEntry};
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::graph::types::DagStatus;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_core::multiagent::session_graph::{attach_runs, snapshot_for_session};

use crate::runtime_storage::SessionRepository;
use crate::session_execution::SessionExecutionRegistry;
use theway_storage::session_graph::{SessionGraphNode, SessionGraphStore};
use theway_transport::testing::empty_sidebar_snapshot;
use theway_transport::transport::SessionOps;
use theway_transport::wire::{
    SessionSummary, WireAgentJobSnapshot, WireCollapseSessionRequest, WireCollapseSessionResponse,
    WireCollapsedSessionNode, WireDagNodeSnapshot, WireDagRunSnapshot, WireNodeResultSnapshot,
    WireSessionFeed, WireSessionGraphNode, WireSessionGraphNodeType, WireSessionGraphState,
    WireSessionInfo, WireSessionLineage, WireSessionRuntime, WireSessionSnapshot,
    epoch_millis_to_rfc3339,
};

/// Builds a fresh, fully-wired [`crate::orchestration::SessionRuntime`] for the session identified by
/// the given id (resume semantics: full id or unique prefix, same as CLI `--resume-id`).
///
/// Async because opening the transcript, restoring per-session DAG state and rehydrating
/// the agent are all IO. The returned runtime keeps the harness and trigger executor
/// together so a session resume cannot retain session-scoped services from the previous
/// session.
pub type SessionFactory = Arc<
    dyn Fn(
            String,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<crate::orchestration::SessionRuntime>>
                    + Send,
            >,
        > + Send
        + Sync,
>;

/// Session lifecycle ops exposed to the transport servers (session-resource-model tasks
/// 3.5/3.6). All ops are repo-backed; `delete` additionally consults the DAG engine for
/// the delete-protection rule.
///
/// `SessionOps` over a cwd-scoped [`SessionRepository`] and the process DAG engine.
pub(crate) struct AppSessionOps {
    repo: Arc<dyn SessionRepository>,
    dag_engine: Arc<DagEngine>,
    cwd: String,
    session_execution: SessionExecutionRegistry,
    subagent_registry: Option<SubagentJobRegistry>,
    session_graph_path: Option<PathBuf>,
}

impl AppSessionOps {
    #[allow(dead_code)] // Used by inline tests; production uses `with_session_graph`.
    pub(crate) fn new(
        repo: Arc<dyn SessionRepository>,
        dag_engine: Arc<DagEngine>,
        cwd: String,
        session_execution: SessionExecutionRegistry,
    ) -> Self {
        Self {
            repo,
            dag_engine,
            cwd,
            session_execution,
            subagent_registry: None,
            session_graph_path: None,
        }
    }

    pub(crate) fn with_session_graph(
        repo: Arc<dyn SessionRepository>,
        dag_engine: Arc<DagEngine>,
        cwd: String,
        session_execution: SessionExecutionRegistry,
        subagent_registry: SubagentJobRegistry,
        session_graph_path: PathBuf,
    ) -> Self {
        Self {
            repo,
            dag_engine,
            cwd,
            session_execution,
            subagent_registry: Some(subagent_registry),
            session_graph_path: Some(session_graph_path),
        }
    }
}

/// Read the latest `session_metadata` custom entry from a session transcript.
async fn read_session_metadata(
    session: &(impl SessionReader + ?Sized),
) -> Result<HashMap<String, String>> {
    let entries = session.find_entries("custom").await?;
    let mut metadata = HashMap::new();
    for entry in entries {
        if entry
            .payload
            .get("customType")
            .and_then(serde_json::Value::as_str)
            != Some("session_metadata")
        {
            continue;
        }
        if let Some(map) = entry
            .payload
            .get("metadata")
            .and_then(|value| serde_json::from_value::<HashMap<String, String>>(value.clone()).ok())
        {
            metadata.extend(map);
        }
    }
    Ok(metadata)
}

/// Persist a metadata map as a new `session_metadata` custom transcript entry.
async fn append_session_metadata(
    session: &(impl SessionStore + ?Sized),
    metadata: &HashMap<String, String>,
) -> Result<()> {
    let id = session.create_entry_id().await?;
    let parent_id = session.get_leaf_id().await?;
    let timestamp = chrono::Utc::now().to_rfc3339();
    let entry = StoredSessionEntry::from_payload(serde_json::json!({
        "type": "custom",
        "id": id,
        "parentId": parent_id,
        "timestamp": timestamp,
        "customType": "session_metadata",
        "metadata": metadata,
    }))?;
    session.append_entry(entry).await?;
    Ok(())
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn compact_context_entries(
    source_session_id: &str,
    compact_text: &str,
    raw_text_ref: &str,
    graph_state: &theway_core::multiagent::session_graph::SessionGraphState,
) -> Result<Vec<StoredSessionEntry>> {
    let timestamp = now_rfc3339();
    let first_id = uuid::Uuid::now_v7().to_string();
    let second_id = uuid::Uuid::now_v7().to_string();
    let compact = StoredSessionEntry::from_payload(serde_json::json!({
        "type": "custom",
        "id": first_id,
        "parentId": null,
        "timestamp": timestamp,
        "customType": "compact_context",
        "data": {
            "sourceSessionId": source_session_id,
            "compactText": compact_text,
            "rawTextRef": raw_text_ref,
            "collapsedAt": timestamp,
        },
    }))?;
    let mut graph_data = serde_json::to_value(graph_state)?;
    if let Some(object) = graph_data.as_object_mut() {
        object
            .entry("dags")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        object
            .entry("subagents")
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    }
    let graph = StoredSessionEntry::session_graph_state(
        second_id,
        Some(first_id.clone()),
        timestamp,
        graph_data,
    )?;
    Ok(vec![compact, graph])
}

fn storage_node_to_wire(node: &SessionGraphNode, session_id: &str) -> WireSessionGraphNode {
    WireSessionGraphNode {
        id: node.id.clone(),
        session_id: session_id.to_string(),
        node_type: match node.node_type.as_str() {
            "collapsed" => WireSessionGraphNodeType::Collapsed,
            "session" => WireSessionGraphNodeType::Session,
            _ => WireSessionGraphNodeType::Unspecified,
        },
        title: node.name.clone(),
        summary: node.summary.clone().unwrap_or_default(),
        parent_node_id: node.parent_id.clone(),
        child_node_ids: node.child_ids.clone(),
        collapsed_session_id: node.source_session_id.clone(),
        collapsed_at: Some(node.created_at.clone()),
        created_at: Some(node.created_at.clone()),
        updated_at: node.updated_at.clone(),
        message_count: 0,
    }
}

fn make_collapse_node(
    node_id: &str,
    _child_session_id: &str,
    source_session_id: &str,
    title: &str,
    summary: &str,
    graph_state: &theway_core::multiagent::session_graph::SessionGraphState,
) -> SessionGraphNode {
    let now = now_rfc3339();
    SessionGraphNode {
        id: node_id.to_string(),
        node_type: "collapsed".to_string(),
        parent_id: None,
        name: title.to_string(),
        status: "collapsed".to_string(),
        summary: Some(summary.to_string()),
        raw_text_ref: Some(source_session_id.to_string()),
        source_session_id: Some(source_session_id.to_string()),
        run_id: None,
        node_id: None,
        job_id: None,
        subagent_graph: serde_json::to_value(graph_state).unwrap_or_default(),
        child_ids: Vec::new(),
        created_at: now.clone(),
        updated_at: Some(now),
    }
}

fn persisted_run_to_wire(run: &theway_contract::dag::PersistedRun) -> WireDagRunSnapshot {
    let status = if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Running)
    {
        "running"
    } else if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Failed)
    {
        "failed"
    } else if run
        .nodes
        .iter()
        .any(|n| n.status == theway_contract::dag::NodeStatus::Pending)
    {
        "running"
    } else {
        "completed"
    };
    WireDagRunSnapshot {
        id: run.id.clone(),
        name: run.name.clone(),
        kind: run.kind.as_str().to_string(),
        status: status.to_string(),
        fail_fast: run.fail_fast,
        max_concurrency: run.max_concurrency,
        direction: match run.direction {
            theway_contract::dag::Direction::Td => "TD".to_string(),
            theway_contract::dag::Direction::Lr => "LR".to_string(),
        },
        created_at: run.created_at,
        completed_at: run.nodes.iter().filter_map(|n| n.completed_at).max(),
        error: run.nodes.iter().find_map(|n| n.error.clone()),
        nodes: run.nodes.iter().map(persisted_node_to_wire).collect(),
    }
}

fn persisted_node_to_wire(node: &theway_contract::dag::PersistedNode) -> WireDagNodeSnapshot {
    WireDagNodeSnapshot {
        id: node.id.clone(),
        agent: node.agent.clone(),
        status: match node.status {
            theway_contract::dag::NodeStatus::Pending => "pending",
            theway_contract::dag::NodeStatus::Ready => "ready",
            theway_contract::dag::NodeStatus::Running => "running",
            theway_contract::dag::NodeStatus::Succeeded => "succeeded",
            theway_contract::dag::NodeStatus::Failed => "failed",
            theway_contract::dag::NodeStatus::Skipped => "skipped",
            theway_contract::dag::NodeStatus::Cancelled => "cancelled",
        }
        .to_string(),
        depends_on: node.depends_on.clone(),
        job_id: None,
        attempt: node.attempt,
        started_at: node.started_at,
        completed_at: node.completed_at,
        error: node.error.clone(),
        input_tokens: node.input_tokens,
        output_tokens: node.output_tokens,
        result: node.result.as_ref().map(|r| WireNodeResultSnapshot {
            success: r.success,
            error: r.error.clone(),
            duration_ms: r.duration_ms,
            attempt: r.attempt,
            total_attempts: r.total_attempts,
        }),
        output_tail: node.output.clone(),
        live_preview: node.live_preview.clone(),
    }
}

fn subagent_snapshot_to_wire(job: &theway_core::SubagentJobSnapshot) -> WireAgentJobSnapshot {
    WireAgentJobSnapshot {
        id: job.id.clone(),
        agent: job.agent.clone(),
        source: job.source.clone(),
        run_id: job.run_id.clone(),
        node_id: job.node_id.clone(),
        status: job.status.clone(),
        started_at: job.started_at,
        completed_at: job.completed_at,
        duration_ms: job
            .completed_at
            .zip(job.started_at)
            .map(|(end, start)| (end - start).max(0) as u64),
        attempt: job.attempt,
        total_attempts: job.total_attempts,
        input_tokens: Some(job.input_tokens),
        output_tokens: Some(job.output_tokens),
        error: job.error.clone(),
        output_tail: Some(job.output_tail.clone()),
        live_preview: job.live_preview.clone(),
        tps: job.tps,
        cps: job.cps,
        chars: Some(job.chars),
        tools_called: Some(job.tools_called),
        turn: Some(job.turn),
    }
}

#[async_trait]
impl SessionOps for AppSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        let runs = self.dag_engine.list_runs();

        let mut summaries = Vec::new();
        for record in self.repo.list().await? {
            let session_runs = runs
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(record.id.as_str()));
            let graph_count = session_runs.clone().count() as u32;
            let active_graph_count = session_runs
                .filter(|run| run.status == DagStatus::Running)
                .count() as u32;

            let metadata = match self.repo.open(&record.id).await? {
                Some(session) => read_session_metadata(session.as_ref()).await?,
                None => HashMap::new(),
            };
            summaries.push(SessionSummary {
                session_id: record.id,
                name: record.name.unwrap_or_default(),
                cwd: record.cwd,
                model: record.model,
                created_at: record.created_at,
                last_activity_at: record.last_activity_at,
                last_activity_at_rfc3339: epoch_millis_to_rfc3339(record.last_activity_at),
                graph_count,
                active_graph_count,
                busy: false,
                preview: record.preview,
                metadata,
            });
        }
        Ok(summaries)
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        // New sessions record the daemon work_dir so activation can resolve the
        // matching execution context.
        let cwd = if self.cwd.is_empty() {
            ".".to_string()
        } else {
            self.cwd.clone()
        };
        let session = self
            .repo
            .create_with_id(std::path::Path::new(&cwd), session_id)
            .await?;
        let meta = session.get_metadata_json().await?;
        let id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if !metadata.is_empty() {
            append_session_metadata(session.as_ref(), metadata).await?;
        }
        Ok(id)
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let mut merged = read_session_metadata(session.as_ref()).await?;
        merged.extend(metadata.iter().map(|(k, v)| (k.clone(), v.clone())));
        append_session_metadata(session.as_ref(), &merged).await?;
        Ok(())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("session name must not be empty");
        }
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        theway_storage::session::append_session_name(session.as_ref(), name).await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        let session = self
            .repo
            .open(id)
            .await?
            .with_context(|| format!("no session matches id {id}"))?;
        let meta = session.get_metadata_json().await?;
        let session_id = meta
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        // Delete protection: refuse while any of this session's DAG runs is still active.
        let active: Vec<String> = self
            .dag_engine
            .list_runs()
            .iter()
            .filter(|run| {
                run.session_id.as_deref() == Some(session_id.as_str())
                    && run.status == DagStatus::Running
            })
            .map(|run| run.id.clone())
            .collect();
        if !active.is_empty() {
            return Ok(active);
        }

        self.repo.delete(id).await?;
        // Credentials are memory-only and session-scoped; deleting the session
        // must also drop its zeroizing secrets and execution context.
        self.session_execution.remove(&session_id);
        Ok(Vec::new())
    }

    async fn collapse_session(
        &self,
        request: &WireCollapseSessionRequest,
    ) -> Result<WireCollapseSessionResponse> {
        self.collapse_session_inner(request, false).await
    }

    async fn get_session_graph_node(
        &self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Option<WireSessionGraphNode>> {
        let path = self
            .session_graph_path
            .as_ref()
            .context("session graph store not configured")?;
        let store = SessionGraphStore::open(path)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(store
            .load_node(node_id)
            .await
            .map_err(anyhow::Error::msg)?
            .map(|node| storage_node_to_wire(&node, session_id)))
    }

    async fn list_session_graph_nodes(
        &self,
        session_id: &str,
    ) -> Result<Vec<WireSessionGraphNode>> {
        let path = self
            .session_graph_path
            .as_ref()
            .context("session graph store not configured")?;
        let store = SessionGraphStore::open(path)
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(store
            .list_nodes()
            .await
            .map_err(anyhow::Error::msg)?
            .into_iter()
            .map(|node| storage_node_to_wire(&node, session_id))
            .collect())
    }

    async fn session_lineage(&self, session_id: &str) -> Result<WireSessionLineage> {
        let session = self
            .repo
            .open(session_id)
            .await?
            .with_context(|| format!("no session matches id {session_id}"))?;
        let meta = session.get_metadata_json().await?;
        let mut lineage = WireSessionLineage::default();
        if meta.get("collapsed").and_then(serde_json::Value::as_bool) == Some(true) {
            lineage.collapsed_from_session_id = Some(session_id.to_string());
        }
        Ok(lineage)
    }

    async fn list_session_graph_node_messages(
        &self,
        session_id: &str,
        node_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<theway_transport::feed::WireFeedBlock>> {
        let source_session_id = if let Some(path) = self.session_graph_path.as_ref() {
            let store = SessionGraphStore::open(path)
                .await
                .map_err(anyhow::Error::msg)?;
            store
                .load_node(node_id)
                .await
                .map_err(anyhow::Error::msg)?
                .and_then(|node| node.source_session_id)
                .unwrap_or_else(|| session_id.to_string())
        } else {
            session_id.to_string()
        };
        let session = self
            .repo
            .open(&source_session_id)
            .await?
            .with_context(|| format!("no session matches id {source_session_id}"))?;
        let entries = session.get_entries().await?;
        let start = offset as usize;
        let end = if limit == 0 {
            entries.len()
        } else {
            (start + limit as usize).min(entries.len())
        };
        Ok(entries[start.min(entries.len())..end]
            .iter()
            .filter_map(|entry| {
                if entry.entry_type != "message" {
                    return None;
                }
                let text = entry
                    .payload
                    .get("message")
                    .and_then(|message| message.get("content"))
                    .map(|content| content.to_string())
                    .unwrap_or_default();
                Some(theway_transport::feed::WireFeedBlock::Plain {
                    text,
                    level: theway_transport::feed::Level::Output,
                    timestamp: None,
                })
            })
            .collect())
    }

    async fn session_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        let session = self
            .repo
            .open(session_id)
            .await?
            .with_context(|| format!("no session matches id {session_id}"))?;
        let meta = session.get_metadata_json().await?;
        let id = meta
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(session_id)
            .to_string();
        let sidebar = empty_sidebar_snapshot();
        let info = WireSessionInfo {
            id: id.clone(),
            name: String::new(),
            cwd: meta
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            created_at: meta
                .get("createdAt")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            last_activity_at: 0,
            last_activity_at_rfc3339: None,
            busy: false,
            preview: None,
            metadata: HashMap::new(),
            graph_count: 0,
            active_graph_count: 0,
            queued_count: 0,
            sidebar,
        };
        let graph_state = match self.subagent_registry.as_ref() {
            Some(jobs) => snapshot_for_session(&self.dag_engine, jobs, &id),
            None => theway_core::multiagent::session_graph::SessionGraphState::default(),
        };
        Ok(WireSessionSnapshot {
            session_id: id.clone(),
            info,
            runtime: WireSessionRuntime {
                model: theway_transport::wire::WireModelRef::default(),
                thinking_level: String::new(),
                supported_thinking_levels: Vec::new(),
                context_usage: theway_transport::wire::WireContextUsage::default(),
                session_context_usage: theway_transport::wire::WireContextUsage::default(),
                tui_max_feed_lines: None,
                model_catalog: Vec::new(),
                latest_trigger_poll: None,
                goal: None,
                control_plane_prompt: None,
                extensions: theway_transport::wire::WireExtensionSnapshot::default(),
            },
            feed: WireSessionFeed {
                blocks: Vec::new(),
                lines: Vec::new(),
                blocks_base: 0,
                lines_base: 0,
                block_patches: Vec::new(),
            },
            graph_state: WireSessionGraphState {
                dags: graph_state.dags.iter().map(persisted_run_to_wire).collect(),
                subagents: graph_state
                    .subagents
                    .iter()
                    .map(subagent_snapshot_to_wire)
                    .collect(),
                nodes: self.list_session_graph_nodes(&id).await?,
                active_node_id: meta
                    .get("collapseNodeId")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            },
            lineage: self.session_lineage(&id).await?,
        })
    }
}

impl AppSessionOps {
    pub(crate) async fn collapse_session_with_adopt(
        &self,
        request: &WireCollapseSessionRequest,
        adopt: bool,
    ) -> Result<WireCollapseSessionResponse> {
        self.collapse_session_inner(request, adopt).await
    }

    async fn collapse_session_inner(
        &self,
        request: &WireCollapseSessionRequest,
        adopt: bool,
    ) -> Result<WireCollapseSessionResponse> {
        let source_id = request.session_id.trim();
        if source_id.is_empty() {
            bail!("source session id must not be empty");
        }
        let source_store = self
            .repo
            .open(source_id)
            .await?
            .with_context(|| format!("no session matches id {source_id}"))?;
        let source = theway_core::Session::from_store(source_store.clone());

        let graph_state = match self.subagent_registry.as_ref() {
            Some(jobs) => snapshot_for_session(&self.dag_engine, jobs, source_id),
            None => source.session_graph_state().await?.unwrap_or_default(),
        };
        let mut graph_data = serde_json::to_value(&graph_state)?;
        if let Some(object) = graph_data.as_object_mut() {
            object
                .entry("dags")
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            object
                .entry("subagents")
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        }
        source
            .append_custom(
                theway_core::SESSION_GRAPH_STATE_CUSTOM_TYPE,
                Some(graph_data),
            )
            .await?;

        let compact_text = match request.summary.clone() {
            Some(summary) if !summary.trim().is_empty() => summary,
            _ => source
                .latest_compaction_summary()
                .await?
                .unwrap_or_default(),
        };
        if !compact_text.is_empty() {
            let leaf = source
                .get_leaf_id()
                .await?
                .unwrap_or_else(|| "collapse".to_string());
            source
                .append_compaction(compact_text.clone(), leaf, 0, None, true)
                .await?;
        }

        let title = request
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| format!("Collapsed {source_id}"));
        let summary = if compact_text.is_empty() {
            title.clone()
        } else {
            compact_text.clone()
        };
        let node_id = format!("node-{}", uuid::Uuid::now_v7());
        let entries = compact_context_entries(source_id, &compact_text, source_id, &graph_state)?;

        let child_store = if let Some(into_id) = request
            .into_session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let child = self
                .repo
                .open(into_id)
                .await?
                .with_context(|| format!("no session matches id {into_id}"))?;
            child.append_entries(entries).await?;
            child.set_collapse_node_id(Some(node_id.clone())).await?;
            source_store
                .set_collapse_node_id(Some(node_id.clone()))
                .await?;
            source_store.set_collapsed(true).await?;
            child
        } else {
            self.repo
                .create_collapsed_child(
                    std::path::Path::new(&self.cwd),
                    source_store.as_ref(),
                    entries,
                    node_id.clone(),
                )
                .await?
        };
        let child_meta = child_store.get_metadata_json().await?;
        let child_id = child_meta
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        let graph_path = self
            .session_graph_path
            .as_ref()
            .context("session graph store not configured")?;
        let store = SessionGraphStore::open(graph_path)
            .await
            .map_err(anyhow::Error::msg)?;
        let storage_node = make_collapse_node(
            &node_id,
            &child_id,
            source_id,
            &title,
            &summary,
            &graph_state,
        );
        store
            .save_node(&storage_node)
            .await
            .map_err(anyhow::Error::msg)?;

        if adopt {
            if let Some(jobs) = self.subagent_registry.as_ref() {
                attach_runs(&self.dag_engine, jobs, source_id, &child_id);
            } else {
                bail!("adopt requires a subagent registry");
            }
        }

        let wire_node = storage_node_to_wire(&storage_node, &child_id);
        let collapsed = WireCollapsedSessionNode {
            node_id: node_id.clone(),
            session_id: source_id.to_string(),
            title: title.clone(),
            summary: summary.clone(),
            message_count: 0,
            collapsed_at: Some(now_rfc3339()),
            collapsed_into_session_id: Some(child_id.clone()),
            collapsed_into_node_id: Some(node_id.clone()),
            original_session_ids: vec![source_id.to_string()],
        };
        Ok(WireCollapseSessionResponse {
            session_id: source_id.to_string(),
            node: Some(wire_node),
            collapsed: Some(collapsed),
        })
    }
}

pub async fn collapse_session_for_command(
    repo: Arc<dyn SessionRepository>,
    cwd: &std::path::Path,
    session_id: &str,
    title: Option<String>,
    adopt: bool,
) -> Result<WireCollapseSessionResponse> {
    let graph_path = theway_contract::config::sessions_dir_for_cwd(cwd).join("session_graph.db");
    let ops = AppSessionOps::with_session_graph(
        repo,
        Arc::new(DagEngine::new()),
        cwd.to_string_lossy().into_owned(),
        SessionExecutionRegistry::new(),
        SubagentJobRegistry::new(),
        graph_path,
    );
    let request = WireCollapseSessionRequest {
        session_id: session_id.to_string(),
        into_session_id: None,
        title,
        summary: None,
    };
    ops.collapse_session_with_adopt(&request, adopt).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use theway_contract::session::SessionReader;
    use theway_storage::sqlite_repo::SqliteSessionRepo;

    fn ops(repo: Arc<dyn SessionRepository>, _current_id: &str) -> AppSessionOps {
        let engine = Arc::new(DagEngine::new());
        AppSessionOps::new(repo, engine, "/cwd".into(), SessionExecutionRegistry::new())
    }

    async fn session_id_of(session: &(impl SessionReader + ?Sized)) -> String {
        session
            .get_metadata_json()
            .await
            .unwrap()
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn list_returns_repo_summaries_without_live_current_flags() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo, &id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].session_id, id);
        assert!(!summaries[0].busy, "no daemon-side current busy flag");
        assert_eq!(summaries[0].graph_count, 0);
        assert_eq!(summaries[0].active_graph_count, 0);
    }

    #[tokio::test]
    async fn create_makes_new_session_with_inherited_cwd() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let first = repo.create("/cwd").await.unwrap();
        let first_id = session_id_of(&first).await;

        let ops = ops(repo.clone(), &first_id);
        let new_id = ops.create(None, &HashMap::new()).await.unwrap();
        assert_ne!(new_id, first_id);
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|s| s.cwd == "/cwd"));
    }

    #[tokio::test]
    async fn rename_round_trips_through_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo, &id);
        ops.rename(&id, "  my session  ").await.unwrap();
        let summaries = ops.list().await.unwrap();
        assert_eq!(summaries[0].name, "my session");

        let err = ops.rename(&id, "   ").await.unwrap_err().to_string();
        assert!(err.contains("must not be empty"), "{err}");
        let err = ops
            .rename("no-such-session", "x")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches"), "{err}");
    }

    #[tokio::test]
    async fn delete_removes_session_when_no_active_graphs() {
        let dir = tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create(work.display().to_string()).await.unwrap();
        let id = session_id_of(&session).await;

        let ops = ops(repo.clone(), &id);
        ops.session_execution
            .set(
                id.clone(),
                theway_contract::session::SessionBinding {
                    client_key: "client-1".into(),
                    runtime: theway_contract::session::SessionRuntimeContext {
                        work_dir: work.display().to_string(),
                        provider: None,
                        model: None,
                        base_url: None,
                        thinking: None,
                    },
                },
            )
            .unwrap();
        ops.session_execution
            .set_credential(&id, "faux", b"sentinel".to_vec())
            .unwrap();
        let active = ops.delete(&id).await.unwrap();
        assert!(active.is_empty(), "no graphs → delete succeeds");
        assert!(ops.list().await.unwrap().is_empty());
        assert!(ops.session_execution.get_credential(&id, "faux").is_none());
        assert!(ops.session_execution.get(&id).is_none());
    }

    #[tokio::test]
    async fn delete_refuses_session_with_active_dag_run() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let session = repo.create("/cwd").await.unwrap();
        let id = session_id_of(&session).await;

        let engine = Arc::new(DagEngine::new());
        // A goal run is a real engine run; stamp it to this session like the goal hook does.
        let run_id = engine.plan_goal("test condition", Some(id.clone()));
        let ops = AppSessionOps::new(
            repo.clone(),
            engine.clone(),
            "/cwd".into(),
            SessionExecutionRegistry::new(),
        );

        let active = ops.delete(&id).await.unwrap();
        assert_eq!(
            active,
            vec![run_id.clone()],
            "active run must refuse the delete"
        );
        assert_eq!(ops.list().await.unwrap().len(), 1, "session must survive");

        // Terminal run → protection lifts.
        engine.cancel_run(&run_id, Some("test cleanup"));
        let active = ops.delete(&id).await.unwrap();
        assert!(active.is_empty(), "aborted run must not block delete");
        assert!(ops.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_with_custom_id_and_metadata_round_trips_through_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let mut metadata = HashMap::new();
        metadata.insert("tenant".to_string(), "acme".to_string());
        metadata.insert("source".to_string(), "workmate".to_string());

        let id = ops.create(Some("custom-session"), &metadata).await.unwrap();
        assert_eq!(id, "custom-session");

        let summaries = ops.list().await.unwrap();
        let summary = summaries
            .iter()
            .find(|s| s.session_id == "custom-session")
            .expect("custom session must be listed");
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("acme")
        );
        assert_eq!(
            summary.metadata.get("source").map(String::as_str),
            Some("workmate")
        );
    }

    #[tokio::test]
    async fn create_with_duplicate_custom_id_returns_already_exists() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        ops.create(Some("dup"), &HashMap::new()).await.unwrap();
        let err = ops
            .create(Some("dup"), &HashMap::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("already exists") || err.contains("exists"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn update_metadata_merges_and_appears_in_list() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let mut initial = HashMap::new();
        initial.insert("tenant".to_string(), "acme".to_string());
        let id = ops.create(Some("meta-session"), &initial).await.unwrap();

        let mut update = HashMap::new();
        update.insert("env".to_string(), "prod".to_string());
        update.insert("tenant".to_string(), "globex".to_string());
        ops.update_metadata(&id, &update).await.unwrap();

        let summary = ops
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.session_id == id)
            .unwrap();
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("globex")
        );
        assert_eq!(
            summary.metadata.get("env").map(String::as_str),
            Some("prod")
        );
    }

    #[tokio::test]
    async fn update_metadata_unknown_session_returns_not_found() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let ops = ops(repo, "current");

        let err = ops
            .update_metadata("missing", &HashMap::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no session matches"), "{err}");
    }

    #[tokio::test]
    async fn metadata_is_persisted_across_ops_instances() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let mut initial = HashMap::new();
        initial.insert("tenant".to_string(), "acme".to_string());

        let first = ops(repo.clone(), "current");
        let id = first
            .create(Some("persistent-meta"), &initial)
            .await
            .unwrap();

        let mut update = HashMap::new();
        update.insert("env".to_string(), "prod".to_string());
        first.update_metadata(&id, &update).await.unwrap();

        // A fresh AppSessionOps must read the same metadata from the repo, not
        // from an in-memory cache.
        let second = ops(repo, "current");
        let summary = second
            .list()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.session_id == id)
            .unwrap();
        assert_eq!(
            summary.metadata.get("tenant").map(String::as_str),
            Some("acme")
        );
        assert_eq!(
            summary.metadata.get("env").map(String::as_str),
            Some("prod")
        );
    }

    #[tokio::test]
    async fn collapse_creates_child_and_registers_graph_node() {
        let dir = tempdir().unwrap();
        let repo = Arc::new(SqliteSessionRepo::new(dir.path()));
        let source = repo.create("/cwd").await.unwrap();
        let source_id = session_id_of(&source).await;
        source
            .append_entry(
                StoredSessionEntry::from_payload(serde_json::json!({
                    "type": "custom",
                    "id": "compact-1",
                    "parentId": null,
                    "timestamp": "2026-08-24T00:00:00Z",
                    "customType": "compact_context",
                    "data": { "text": "old" },
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let graph_path = dir.path().join("session_graph.db");
        let ops = AppSessionOps::with_session_graph(
            repo.clone(),
            Arc::new(DagEngine::new()),
            "/cwd".into(),
            SessionExecutionRegistry::new(),
            SubagentJobRegistry::new(),
            graph_path.clone(),
        );
        let request = WireCollapseSessionRequest {
            session_id: source_id.clone(),
            into_session_id: None,
            title: Some("Archived".into()),
            summary: Some("compact summary".into()),
        };
        let response = ops.collapse_session(&request).await.unwrap();
        let child_id = response
            .collapsed
            .as_ref()
            .unwrap()
            .collapsed_into_session_id
            .clone()
            .unwrap();
        let node_id = response.node.as_ref().unwrap().id.clone();
        assert_ne!(child_id, source_id);
        assert!(response.node.as_ref().unwrap().title.contains("Archived"));

        let store = SessionGraphStore::open(&graph_path).await.unwrap();
        let node = store.load_node(&node_id).await.unwrap().unwrap();
        assert_eq!(node.node_type, "collapsed");
        assert_eq!(node.source_session_id.as_deref(), Some(source_id.as_str()));

        let child = SessionRepository::open(repo.as_ref(), &child_id)
            .await
            .unwrap()
            .unwrap();
        let entries = child.get_entries().await.unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.payload["customType"] == "compact_context"),
            "child must carry compact context entries"
        );
    }
}
