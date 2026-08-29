//! Single `SessionOps` trait implementation for [`super::AppSessionOps`].
//!
//! The `#[async_trait]` impl block cannot be split across modules; each
//! domain module exposes free functions and this facade delegates to them.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use theway_transport::transport::SessionOps;
use theway_transport::wire::{
    SessionSummary, WireCollapseSessionRequest, WireCollapseSessionResponse, WireSessionGraphNode,
    WireSessionLineage, WireSessionSnapshot,
};

use super::{AppSessionOps, collapse, graph, lifecycle};

#[async_trait]
impl SessionOps for AppSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        lifecycle::list(self).await
    }

    async fn create(
        &self,
        session_id: Option<&str>,
        metadata: &HashMap<String, String>,
    ) -> Result<String> {
        lifecycle::create(self, session_id, metadata).await
    }

    async fn update_metadata(&self, id: &str, metadata: &HashMap<String, String>) -> Result<()> {
        lifecycle::update_metadata(self, id, metadata).await
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        lifecycle::rename(self, id, name).await
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        lifecycle::delete(self, id).await
    }

    async fn collapse_session(
        &self,
        request: &WireCollapseSessionRequest,
    ) -> Result<WireCollapseSessionResponse> {
        collapse::collapse_session(self, request).await
    }

    async fn get_session_graph_node(
        &self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Option<WireSessionGraphNode>> {
        graph::get_session_graph_node(self, session_id, node_id).await
    }

    async fn list_session_graph_nodes(
        &self,
        session_id: &str,
    ) -> Result<Vec<WireSessionGraphNode>> {
        graph::list_session_graph_nodes(self, session_id).await
    }

    async fn session_lineage(&self, session_id: &str) -> Result<WireSessionLineage> {
        graph::session_lineage(self, session_id).await
    }

    async fn list_session_graph_node_messages(
        &self,
        session_id: &str,
        node_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<theway_transport::feed::WireFeedBlock>> {
        graph::list_session_graph_node_messages(self, session_id, node_id, offset, limit).await
    }

    async fn session_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        graph::session_snapshot(self, session_id).await
    }
}
