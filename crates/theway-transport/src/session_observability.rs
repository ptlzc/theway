//! Protocol-neutral session observability boundary (single-version snapshot
//! contract). gRPC, JSON-RPC, and MCP adapt every snapshot/history read onto
//! this trait; the daemon composes the authoritative implementation.

use anyhow::Result;
use async_trait::async_trait;

use crate::feed::WireFeedBlock;
use crate::wire::WireSessionSnapshot;

/// Server-side cap for one message-history page.
pub const MAX_SESSION_MESSAGE_PAGE: u32 = 500;

/// Cursor-paginated request for the full message history of a session's
/// active branch.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ListSessionMessagesRequest {
    pub session_id: String,
    /// Omitted = newest page. Otherwise the storage entry id of the newest
    /// message NOT to include (exclusive cursor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_entry_id: Option<String>,
    #[serde(default)]
    pub limit: u32,
}

impl ListSessionMessagesRequest {
    /// Effective page size after applying the server cap.
    pub fn effective_limit(&self) -> u32 {
        self.limit.clamp(1, MAX_SESSION_MESSAGE_PAGE)
    }
}

/// One page of message blocks plus the cursor for the previous page.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct SessionMessagePage {
    pub session_id: String,
    /// Oldest → newest within the page.
    #[serde(default)]
    pub blocks: Vec<WireFeedBlock>,
    /// Entry id of the oldest message in this page; `None` when no older
    /// messages exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_before_entry_id: Option<String>,
    pub has_more: bool,
    pub total: u64,
}

/// Single-version session observability: one authoritative current snapshot
/// plus cursor-paginated full message history.
#[async_trait]
pub trait SessionObservabilityOps: Send + Sync {
    /// Authoritative current snapshot. Live projection fields
    /// (`runtime`/`feed`/`system_context`/`dags`/`subagents`) come from the
    /// live runtime, resource fields (`info`/`graph_state.nodes`/
    /// `active_node_id`/`lineage`) from the session resource plane.
    async fn authoritative_snapshot(&self, session_id: &str) -> Result<WireSessionSnapshot>;

    /// Cursor-paginated full message history on the session's active branch.
    async fn list_session_messages(
        &self,
        request: &ListSessionMessagesRequest,
    ) -> Result<SessionMessagePage>;
}

/// Single failure message for every [`UnavailableSessionObservability`] operation.
pub const SESSION_OBSERVABILITY_UNAVAILABLE: &str =
    "session observability is not wired to this host yet";

/// Placeholder [`SessionObservabilityOps`] for hosts/tests that only exercise
/// unrelated protocol surfaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableSessionObservability;

#[async_trait]
impl SessionObservabilityOps for UnavailableSessionObservability {
    async fn authoritative_snapshot(&self, _session_id: &str) -> Result<WireSessionSnapshot> {
        anyhow::bail!(SESSION_OBSERVABILITY_UNAVAILABLE)
    }

    async fn list_session_messages(
        &self,
        _request: &ListSessionMessagesRequest,
    ) -> Result<SessionMessagePage> {
        anyhow::bail!(SESSION_OBSERVABILITY_UNAVAILABLE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_limit_is_clamped_to_server_cap() {
        let request = ListSessionMessagesRequest {
            session_id: "sess-1".into(),
            before_entry_id: None,
            limit: u32::MAX,
        };
        assert_eq!(request.effective_limit(), MAX_SESSION_MESSAGE_PAGE);

        let request = ListSessionMessagesRequest {
            limit: 0,
            ..request
        };
        assert_eq!(request.effective_limit(), 1);
    }

    #[tokio::test]
    async fn unavailable_observability_fails_every_operation() {
        let ops = UnavailableSessionObservability;
        let error = ops.authoritative_snapshot("sess-1").await.unwrap_err();
        assert_eq!(error.to_string(), SESSION_OBSERVABILITY_UNAVAILABLE);
        let error = ops
            .list_session_messages(&ListSessionMessagesRequest {
                session_id: "sess-1".into(),
                before_entry_id: None,
                limit: 10,
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), SESSION_OBSERVABILITY_UNAVAILABLE);
    }
}
