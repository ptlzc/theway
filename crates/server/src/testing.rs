//! Test fakes for the transport servers (session-resource-model N4).
//!
//! [`FakeSessionOps`] is an in-memory [`theway::session_ops::SessionOps`] so the gRPC/HTTP
//! session tests exercise the transport surface without a real `JsonlSessionRepo` on disk.
//! Delete protection is simulated by mapping a session id to its "running" run ids.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use theway::wire::SessionSummary;

/// In-memory `SessionOps`: sessions live in a `Vec` (oldest → newest, like the repo-backed
/// impl), ids for `create` come from a counter.
#[derive(Default)]
pub(crate) struct FakeSessionOps {
    inner: Mutex<FakeInner>,
}

#[derive(Default)]
struct FakeInner {
    sessions: Vec<SessionSummary>,
    counter: u64,
    /// session_id → running run ids; non-empty refuses `delete`.
    running: HashMap<String, Vec<String>>,
}

impl FakeSessionOps {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Seed an existing session; returns its id.
    pub(crate) fn add_session(&self, id: &str) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.push(summary(id));
        id.to_string()
    }

    /// Mark a session as having running graphs (blocks `delete`, ids reported back).
    pub(crate) fn set_running(&self, session_id: &str, run_ids: &[&str]) {
        let mut inner = self.inner.lock().unwrap();
        inner.running.insert(
            session_id.to_string(),
            run_ids.iter().map(|s| s.to_string()).collect(),
        );
    }
}

fn summary(id: &str) -> SessionSummary {
    SessionSummary {
        session_id: id.to_string(),
        name: String::new(),
        cwd: "/tmp/theway".to_string(),
        model: "provider:model".to_string(),
        created_at: String::new(),
        last_activity_at: 0,
        graph_count: 0,
        active_graph_count: 0,
        busy: false,
        preview: None,
    }
}

#[tonic::async_trait]
impl theway::session_ops::SessionOps for FakeSessionOps {
    async fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(self.inner.lock().unwrap().sessions.clone())
    }

    async fn create(&self) -> Result<String> {
        let mut inner = self.inner.lock().unwrap();
        inner.counter += 1;
        let id = format!("sess-new-{}", inner.counter);
        inner.sessions.push(summary(&id));
        Ok(id)
    }

    async fn rename(&self, id: &str, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("session name must not be empty");
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(session) = inner.sessions.iter_mut().find(|s| s.session_id == id) else {
            anyhow::bail!("no session matches id {id}");
        };
        session.name = name.to_string();
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<Vec<String>> {
        let mut inner = self.inner.lock().unwrap();
        let Some(pos) = inner.sessions.iter().position(|s| s.session_id == id) else {
            anyhow::bail!("no session matches id {id}");
        };
        if let Some(runs) = inner.running.get(id)
            && !runs.is_empty()
        {
            return Ok(runs.clone());
        }
        inner.sessions.remove(pos);
        inner.running.remove(id);
        Ok(Vec::new())
    }
}
