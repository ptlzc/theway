# -*- coding: utf-8 -*-
p = 'C:/Users/lizc/Workspace/theway/crates/harness/src/transport/grpc.rs'
s = open(p, encoding='utf-8').read()

# 1. GrpcState: bring back session_id (needed as default for checkpoint scope)
old = '''    /// DAG orchestration engine (graph engineering mode): GraphCancel/Retry/…
    dag_engine: Arc<theway_core::harness::graph_engineering::engine::DagEngine>,
}'''
new = '''    /// DAG orchestration engine (graph engineering mode): GraphCancel/Retry/…
    dag_engine: Arc<theway_core::harness::graph_engineering::engine::DagEngine>,
    /// Owning session id: default scope for GraphCheckpoint and the mount key
    /// under which `SessionState.dags` is served.
    session_id: String,
}'''
assert old in s, "GrpcState"
s = s.replace(old, new)

# 2. construction
old = '''            dag_engine: self.dag_engine.clone(),
        };'''
new = '''            dag_engine: self.dag_engine.clone(),
            session_id: self.session_id.clone(),
        };'''
assert old in s, "grpc_state construction"
s = s.replace(old, new)

old = '''                dag_engine: Arc::new(
                    theway_core::harness::graph_engineering::engine::DagEngine::new(),
                ),
            },'''
new = '''                dag_engine: Arc::new(
                    theway_core::harness::graph_engineering::engine::DagEngine::new(),
                ),
                session_id: "test-session".into(),
            },'''
assert old in s, "test construction"
s = s.replace(old, new)

# 3. graph_checkpoint: session-scoped batch export
old = '''    async fn graph_checkpoint(
        &self,
        request: Request<GraphCheckpointRequest>,
    ) -> Result<Response<GraphCheckpointResponse>, Status> {
        use theway_core::harness::graph_engineering::persist::to_persisted;
        let run_id = request.into_inner().run_id;
        let Some(run) = self.dag_engine.get_run(&run_id) else {
            return Ok(Response::new(GraphCheckpointResponse {
                kind: GraphKind::GraphDag as i32,
                snapshot: String::new(),
                error: Some(format!("unknown run: {run_id}")),
            }));
        };
        let persisted = to_persisted(&run);
        let snapshot = serde_json::to_string(&persisted).map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(GraphCheckpointResponse {
            kind: match run.kind {
                theway_core::harness::graph_engineering::types::RunKind::Goal => {
                    GraphKind::GraphGoal as i32
                }
                _ => GraphKind::GraphDag as i32,
            },
            snapshot,
            error: None,
        }))
    }

    async fn graph_restore(
        &self,
        request: Request<GraphRestoreRequest>,
    ) -> Result<Response<GraphRestoreResponse>, Status> {
        let snapshot = request.into_inner().snapshot;
        let persisted: theway_core::harness::graph_engineering::persist::PersistedRun =
            serde_json::from_str(&snapshot)
                .map_err(|e| Status::invalid_argument(format!("invalid snapshot: {e}")))?;
        let ids = self.dag_engine.restore(vec![persisted]);
        let Some(run_id) = ids.first() else {
            return Ok(Response::new(GraphRestoreResponse {
                run_id: String::new(),
                error: Some("restore produced no run".into()),
            }));
        };
        Ok(Response::new(GraphRestoreResponse {
            run_id: run_id.clone(),
            error: None,
        }))
    }'''
new = '''    async fn graph_checkpoint(
        &self,
        request: Request<GraphCheckpointRequest>,
    ) -> Result<Response<GraphCheckpointResponse>, Status> {
        use theway_core::harness::graph_engineering::persist::to_persisted;
        use theway_core::harness::graph_engineering::types::RunKind;
        let request = request.into_inner();
        let session_id = request
            .session_id
            .clone()
            .unwrap_or_else(|| self.session_id.clone());

        // Single-run export, or every run owned by the session.
        let runs: Vec<theway_core::harness::graph_engineering::types::DagRun> =
            match request.run_id {
                Some(run_id) => self
                    .dag_engine
                    .get_run(&run_id)
                    .into_iter()
                    .filter(|r| r.session_id.as_deref().is_none_or(|sid| sid == session_id))
                    .collect(),
                None => self
                    .dag_engine
                    .list_runs()
                    .into_iter()
                    .filter(|r| r.session_id.as_deref().is_none_or(|sid| sid == session_id))
                    .collect(),
            };

        let mut checkpoints = Vec::new();
        for run in &runs {
            let persisted = to_persisted(run);
            let snapshot = serde_json::to_string(&persisted)
                .map_err(|e| Status::internal(e.to_string()))?;
            checkpoints.push(theway_grpc::GraphSnapshotEntry {
                kind: match run.kind {
                    RunKind::Goal => GraphKind::GraphGoal as i32,
                    _ => GraphKind::GraphDag as i32,
                },
                run_id: run.id.clone(),
                snapshot,
            });
        }
        let error = if runs.is_empty() {
            Some(format!("no runs for session {session_id}"))
        } else {
            None
        };
        Ok(Response::new(GraphCheckpointResponse {
            session_id,
            checkpoints,
            error,
        }))
    }

    async fn graph_restore(
        &self,
        request: Request<GraphRestoreRequest>,
    ) -> Result<Response<GraphRestoreResponse>, Status> {
        let request = request.into_inner();
        let mut persisted: theway_core::harness::graph_engineering::persist::PersistedRun =
            serde_json::from_str(&request.snapshot)
                .map_err(|e| Status::invalid_argument(format!("invalid snapshot: {e}")))?;
        // Re-attach to the requesting session (snapshots are portable).
        persisted.session_id = Some(request.session_id.clone());
        let ids = self.dag_engine.restore(vec![persisted]);
        let Some(run_id) = ids.first() else {
            return Ok(Response::new(GraphRestoreResponse {
                run_id: String::new(),
                error: Some("restore produced no run".into()),
            }));
        };
        Ok(Response::new(GraphRestoreResponse {
            run_id: run_id.clone(),
            error: None,
        }))
    }'''
assert old in s, "checkpoint/restore impls"
s = s.replace(old, new)
open(p, 'w', encoding='utf-8', newline='').write(s)
print("grpc.rs session-scoped checkpoint/restore OK")
