//! Session collapse implementation and the CLI collapse entry point.

use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use theway_contract::session::SessionReader;
use theway_core::multiagent::graph::engine::DagEngine;
use theway_core::multiagent::jobs::SubagentJobRegistry;
use theway_core::multiagent::session_graph::{attach_runs, snapshot_for_session};
use theway_storage::session_graph::SessionGraphStore;
use theway_transport::wire::{
    WireCollapseSessionRequest, WireCollapseSessionResponse, WireCollapsedSessionNode,
};

use crate::runtime_storage::SessionRepository;
use crate::session_execution::SessionExecutionRegistry;

use super::AppSessionOps;
use super::metadata::{
    compact_context_entries, now_rfc3339, render_rolling_summary, transcript_material,
};
use super::wire::{make_collapse_node, storage_node_to_wire};

pub(super) async fn collapse_session(
    ops: &AppSessionOps,
    request: &WireCollapseSessionRequest,
) -> Result<WireCollapseSessionResponse> {
    ops.collapse_session_inner(request, false).await
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
        // Capture the source's existing collapse node BEFORE child creation
        // overwrites `collapseNodeId`: it is the parent link in the node chain.
        let parent_node_id = source.collapse_node_id().await?;

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

        let compact_material = match request.summary.clone() {
            Some(summary) if !summary.trim().is_empty() => summary,
            _ => match source.latest_collapse_summary().await? {
                Some(summary) => summary,
                None => transcript_material(&source).await?,
            },
        };
        let compact_text = render_rolling_summary(&compact_material);
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
            let leaf = child.get_leaf_id().await?;
            let entries = compact_context_entries(
                source_id,
                &compact_text,
                source_id,
                &graph_state,
                leaf.as_deref(),
            )?;
            let last_id = entries
                .last()
                .map(|entry| entry.id.clone())
                .unwrap_or_default();
            child.append_entries(entries).await?;
            if !last_id.is_empty() {
                child.set_leaf_id(Some(last_id)).await?;
            }
            child.set_collapse_node_id(Some(node_id.clone())).await?;
            source_store
                .set_collapse_node_id(Some(node_id.clone()))
                .await?;
            source_store.set_collapsed(true).await?;
            child
        } else {
            let entries =
                compact_context_entries(source_id, &compact_text, source_id, &graph_state, None)?;
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
            parent_node_id.as_deref(),
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
    let graph_path = theway_contract::config::sessions_dir_for_cwd(cwd)
        .join(theway_storage::session_graph::SESSION_GRAPH_DB_FILE);
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
