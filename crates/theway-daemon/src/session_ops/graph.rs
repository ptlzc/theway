//! Session graph node, lineage, message, and snapshot query ops.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use theway_core::multiagent::session_graph::snapshot_for_session;
use theway_storage::session_graph::SessionGraphStore;
use theway_transport::testing::empty_sidebar_snapshot;
use theway_transport::wire::{
    WireSessionFeed, WireSessionGraphNode, WireSessionGraphState, WireSessionInfo,
    WireSessionLineage, WireSessionRuntime, WireSessionSnapshot,
};

use super::AppSessionOps;
use super::wire::{persisted_run_to_wire, storage_node_to_wire, subagent_snapshot_to_wire};

pub(super) async fn get_session_graph_node(
    ops: &AppSessionOps,
    session_id: &str,
    node_id: &str,
) -> Result<Option<WireSessionGraphNode>> {
    let path = ops
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

pub(super) async fn list_session_graph_nodes(
    ops: &AppSessionOps,
    session_id: &str,
) -> Result<Vec<WireSessionGraphNode>> {
    let path = ops
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

pub(super) async fn session_lineage(
    ops: &AppSessionOps,
    session_id: &str,
) -> Result<WireSessionLineage> {
    let session = ops
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

pub(super) async fn list_session_graph_node_messages(
    ops: &AppSessionOps,
    session_id: &str,
    node_id: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<theway_transport::feed::WireFeedBlock>> {
    let source_session_id = if let Some(path) = ops.session_graph_path.as_ref() {
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
    let session = ops
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

pub(super) async fn session_snapshot(
    ops: &AppSessionOps,
    session_id: &str,
) -> Result<WireSessionSnapshot> {
    let session = ops
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
    let graph_state = match ops.subagent_registry.as_ref() {
        Some(jobs) => snapshot_for_session(&ops.dag_engine, jobs, &id),
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
            shell_count: 0,
            model_catalog: Vec::new(),
            latest_trigger_poll: None,
            goal: None,
            control_plane_prompt: None,
            extensions: theway_transport::wire::WireExtensionSnapshot::default(),
            system_context: String::new(),
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
            nodes: list_session_graph_nodes(ops, &id).await?,
            active_node_id: meta
                .get("collapseNodeId")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        },
        lineage: session_lineage(ops, &id).await?,
    })
}
