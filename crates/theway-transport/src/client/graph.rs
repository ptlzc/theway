impl GrpcClient {
    pub async fn graph_cancel(&mut self, run_id: &str) -> Result<bool> {
        let accepted = self
            .graph
            .graph_cancel(GraphCancelRequest {
                run_id: run_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_cancel: {e}"))?;
        Ok(accepted.into_inner().accepted)
    }

    pub async fn graph_retry(
        &mut self,
        run_id: &str,
        node_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let response = self
            .graph
            .graph_retry(GraphRetryRequest {
                run_id: run_id.to_string(),
                node_id: node_id.map(str::to_string),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_retry: {e}"))?;
        Ok(response.into_inner().reset_node_ids)
    }

    pub async fn graph_skip(&mut self, run_id: &str, node_id: &str) -> Result<bool> {
        let skipped = self
            .graph
            .graph_skip(GraphSkipRequest {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("graph_skip: {e}"))?;
        Ok(skipped.into_inner().skipped)
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
        run_id: &str,
        node_id: &str,
        offset: u64,
    ) -> Result<proto::GetNodeOutputResponse> {
        let response = self
            .graph
            .get_node_output(GetNodeOutputRequest {
                run_id: run_id.to_string(),
                node_id: node_id.to_string(),
                offset,
            })
            .await
            .map_err(|e| anyhow::anyhow!("get_node_output: {e}"))?;
        Ok(response.into_inner())
    }
}
