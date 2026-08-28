impl GrpcClient {
    pub async fn graph_cancel(&mut self, session_id: &str, run_id: &str) -> Result<bool> {
        let accepted = self
            .graph
            .graph_cancel(GraphCancelRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    pub async fn graph_retry(
        &mut self,
        session_id: &str,
        run_id: &str,
        node_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let response = self
            .graph
            .graph_retry(GraphRetryRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_retry: {e}"))?;
        Ok(response.into_inner().reset_node_ids)
    }

    pub async fn graph_skip(
        &mut self,
        session_id: &str,
        run_id: &str,
        node_id: &str,
    ) -> Result<bool> {
        let skipped = self
            .graph
            .graph_skip(GraphSkipRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_skip: {e}"))?;
        Ok(skipped.into_inner().skipped)
    }

    pub async fn graph_node_interrupt(
        &mut self,
        session_id: &str,
        run_id: &str,
        node_id: &str,
    ) -> Result<bool> {
        let accepted = self
            .graph
            .graph_node_interrupt(GraphNodeInterruptRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_node_interrupt: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    pub async fn graph_node_steer(
        &mut self,
        session_id: &str,
        run_id: &str,
        node_id: &str,
        text: String,
    ) -> Result<bool> {
        let accepted = self
            .graph
            .graph_node_steer(GraphNodeSteerRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
                text,
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_node_steer: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    /// Export graph checkpoints for a session.
    pub async fn graph_checkpoint(
        &mut self,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<proto::GraphCheckpointResponse> {
        let response = self
            .graph
            .graph_checkpoint(GraphCheckpointRequest {
                session_id: session_id.map(str::to_string),
                run_id: run_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_checkpoint: {e}"))?;
        Ok(response.into_inner())
    }

    /// Restore a graph run from a checkpoint snapshot.
    pub async fn graph_restore(
        &mut self,
        session_id: &str,
        snapshot: String,
    ) -> Result<proto::GraphRestoreResponse> {
        let response = self
            .graph
            .graph_restore(GraphRestoreRequest {
                session_id: session_id.to_string(),
                snapshot,
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_restore: {e}"))?;
        Ok(response.into_inner())
    }

    /// One session's graph runs (DagRunSnapshot shape).
    pub async fn graph_list(&mut self, session_id: &str) -> Result<Vec<proto::DagRunSnapshot>> {
        let response = self
            .graph
            .graph_list(GraphListRequest {
                session_id: session_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_list: {e}"))?;
        Ok(response.into_inner().runs)
    }

    /// A DAG node's full output text from an offset.
    pub async fn get_node_output(
        &mut self,
        session_id: &str,
        run_id: &str,
        node_id: &str,
        offset: u64,
    ) -> Result<proto::GetNodeOutputResponse> {
        let response = self
            .graph
            .get_node_output(GetNodeOutputRequest {
                session_id: session_id.to_string(),
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
                offset,
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_node_output: {e}"))?;
        Ok(response.into_inner())
    }

    // ── session graph nodes (session-snapshot-collapse) ───────────────

    /// Fetch one session graph node.
    pub async fn get_session_graph_node(
        &mut self,
        session_id: &str,
        node_id: &str,
    ) -> Result<proto::SessionGraphNode> {
        let response = self
            .session
            .get_session_graph_node(GetSessionGraphNodeRequest {
                session_id: session_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_session_graph_node: {e}"))?
            .into_inner();
        Ok(response
            .node
            .expect("get_session_graph_node returned no node"))
    }

    /// List all messages attached to a session graph node (first page).
    ///
    /// The daemon treats `offset=0, limit=0` as "server default page" so this
    /// convenience call remains useful for small transcripts.
    pub async fn list_session_graph_node_messages(
        &mut self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Vec<proto::FeedBlock>> {
        self.list_session_graph_node_messages_page(session_id, node_id, 0, 0)
            .await
    }

    /// List a page of messages attached to a session graph node.
    pub async fn list_session_graph_node_messages_page(
        &mut self,
        session_id: &str,
        node_id: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<proto::FeedBlock>> {
        let response = self
            .session
            .list_session_graph_node_messages(ListSessionGraphNodeMessagesRequest {
                session_id: session_id.to_string(),
                node_id: node_id.to_string(),
                offset,
                limit,
            })
            .await
            .map_err(|e| anyhow::anyhow!("list_session_graph_node_messages: {e}"))?
            .into_inner();
        Ok(response.blocks)
    }

    /// Open a streaming session-graph-node frame stream.
    pub async fn stream_session_graph_node(
        &mut self,
        session_id: &str,
        node_id: &str,
    ) -> Result<Streaming<proto::SessionGraphNodeStreamFrame>> {
        let response = self
            .session
            .stream_session_graph_node(StreamSessionGraphNodeRequest {
                session_id: session_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("stream_session_graph_node: {e}"))?;
        Ok(response.into_inner())
    }
}
