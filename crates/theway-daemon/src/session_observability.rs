//! Daemon implementation of the single-version session observability seam:
//! [`DaemonSessionObservability`] merges the live runtime projection with the
//! session resource plane and paginates the full message history.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use theway_core::{Session, SessionTreeEntry};
use theway_transport::session_observability::{
    ListSessionMessagesRequest, SessionMessagePage, SessionObservabilityOps,
};
use theway_transport::transport::SessionOps;
use theway_transport::wire::{WireSessionSnapshot, WireStatus};

use crate::feed_replay::session_tree_entry_wire_blocks;
use crate::runtime_storage::SessionRepository;

pub(crate) struct DaemonSessionObservability {
    session_ops: Arc<dyn SessionOps>,
    session_states: Arc<Mutex<HashMap<String, WireStatus>>>,
    latest: Arc<Mutex<WireStatus>>,
    repo: Arc<dyn SessionRepository>,
}

impl DaemonSessionObservability {
    pub(crate) fn new(
        session_ops: Arc<dyn SessionOps>,
        session_states: Arc<Mutex<HashMap<String, WireStatus>>>,
        latest: Arc<Mutex<WireStatus>>,
        repo: Arc<dyn SessionRepository>,
    ) -> Self {
        Self {
            session_ops,
            session_states,
            latest,
            repo,
        }
    }

    fn live_status(&self, session_id: &str) -> Option<WireStatus> {
        self.session_states
            .lock()
            .get(session_id)
            .cloned()
            .or_else(|| {
                let latest = self.latest.lock();
                (latest.session_id == session_id).then(|| latest.clone())
            })
    }
}

#[async_trait]
impl SessionObservabilityOps for DaemonSessionObservability {
    async fn authoritative_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot> {
        // 1. Resource snapshot: session identity/info, graph nodes, lineage.
        let resource = self
            .session_ops
            .session_snapshot(session_id)
            .await
            .with_context(|| format!("load resource snapshot for session {session_id}"));

        // 2. Live projection (per-session map first, then the active latest).
        let Some(live) = self.live_status(session_id) else {
            // No live projection: this is a cold /resume or explicit-session
            // read. Seed the resource snapshot with the persisted transcript
            // so the client sees the conversation immediately; the full
            // runtime is still built lazily on the first message.
            let mut resource = resource?;
            if let Ok(page) = self
                .list_session_messages(&ListSessionMessagesRequest {
                    session_id: session_id.to_string(),
                    before_entry_id: None,
                    limit: u32::MAX,
                })
                .await
            {
                resource.feed.blocks = page.blocks;
            }
            return Ok(resource);
        };

        let Ok(mut resource) = resource else {
            // Lazy sessions (issue #46) have a live runtime but no materialized
            // repository record yet; project from live until the first write.
            return Ok(WireSessionSnapshot::from(&live));
        };

        // 3. Merge: runtime / feed / system_context / dags / subagents come
        //    from live; info / graph nodes / active node / lineage come from
        //    the resource plane.
        let mut merged = WireSessionSnapshot::from(&live);
        merged.session_id = if resource.session_id.is_empty() {
            merged.session_id
        } else {
            resource.session_id.clone()
        };
        merged.info = resource.info;
        merged.graph_state.nodes = std::mem::take(&mut resource.graph_state.nodes);
        merged.graph_state.active_node_id = resource.graph_state.active_node_id.take();
        merged.lineage = resource.lineage;
        Ok(merged)
    }

    async fn list_session_messages(
        &self,
        request: &ListSessionMessagesRequest,
    ) -> Result<SessionMessagePage> {
        let limit = request.effective_limit() as usize;
        let store = self
            .repo
            .open(&request.session_id)
            .await?
            .with_context(|| format!("no session matches id {}", request.session_id))?;
        let session = Session::from_store(store);
        let branch = session.branch(None).await?;
        let messages: Vec<&SessionTreeEntry> = branch
            .iter()
            .filter(|entry| matches!(entry, SessionTreeEntry::Message { .. }))
            .collect();
        let total = messages.len() as u64;

        // Cursor semantics: `before_entry_id` is exclusive. Unknown cursor →
        // empty page (the client's cursor predates a fork/compaction).
        let before = match request.before_entry_id.as_deref() {
            Some(id) => match messages.iter().position(|entry| entry.id() == id) {
                Some(position) => position,
                None => {
                    return Ok(SessionMessagePage {
                        session_id: request.session_id.clone(),
                        blocks: Vec::new(),
                        next_before_entry_id: None,
                        has_more: false,
                        total,
                    });
                }
            },
            None => messages.len(),
        };

        let start = before.saturating_sub(limit);
        let page = &messages[start..before];
        let mut blocks = Vec::new();
        for entry in page {
            if let Some(entry_blocks) = session_tree_entry_wire_blocks(entry) {
                blocks.extend(entry_blocks);
            }
        }
        Ok(SessionMessagePage {
            session_id: request.session_id.clone(),
            blocks,
            next_before_entry_id: page.first().map(|entry| entry.id().to_string()),
            has_more: start > 0,
            total,
        })
    }
}

#[cfg(test)]
// Test files live in `tests/session_observability/` (mirror of src), pulled in
// by path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("session_observability");
