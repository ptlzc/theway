//! Test fakes for the transport servers (session-resource-model N4).
//!
//! [`FakeSessionOps`] is an in-memory [`crate::transport::SessionOps`] so the gRPC/HTTP
//! session tests exercise the transport surface without a real session repo on disk.
//! Delete protection is simulated by mapping a session id to its "running" run ids.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::wire::{
    SessionSummary, WireCronSnapshot, WireMcpSnapshot, WireSidebarSnapshot, WireSkillsSnapshot,
    WireToolsSnapshot, WireTriggersSnapshot,
};
use anyhow::Result;
use async_trait::async_trait;

/// In-memory `SessionOps`: sessions live in a `Vec` (oldest → newest, like the repo-backed
/// impl), ids for `create` come from a counter.
#[derive(Default)]
pub struct FakeSessionOps {
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed an existing session; returns its id.
    pub fn add_session(&self, id: &str) -> String {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.push(summary(id));
        id.to_string()
    }

    /// Mark a session as having running graphs (blocks `delete`, ids reported back).
    pub fn set_running(&self, session_id: &str, run_ids: &[&str]) {
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

#[async_trait]
impl crate::transport::SessionOps for FakeSessionOps {
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

/// Minimal sidebar used by snapshot fixtures (transport tests + client tests).
pub fn empty_sidebar_snapshot() -> WireSidebarSnapshot {
    WireSidebarSnapshot {
        inbox_new: crate::inbox::new_count(&crate::inbox::default_inbox_path()),
        skills: WireSkillsSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            builtin: 0,
            user: 0,
            project: 0,
            items: Vec::new(),
        },
        triggers: WireTriggersSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            rules: Vec::new(),
        },
        cron: WireCronSnapshot {
            total: 0,
            enabled: 0,
            disabled: 0,
            jobs: Vec::new(),
        },
        mcp: WireMcpSnapshot {
            servers: 0,
            tools: 0,
            notification_hooks: 0,
            server_names: Vec::new(),
            tool_names: Vec::new(),
        },
        tools: WireToolsSnapshot {
            total: 0,
            names: Vec::new(),
        },
        hooks: Vec::new(),
        runtime: Vec::new(),
        commands: Vec::new(),
    }
}
