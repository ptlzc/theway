//! Goal + session state (`App` methods split out of `ui/mod.rs`).
//!
//! Client mode: goal state arrives in snapshots (`latest.goal`); sessions are
//! addressed explicitly by session id, and control-plane resolution is an RPC
//! call. The import-activation card is resolved locally (it writes sidecar
//! files, no daemon round-trip).

use anyhow::Result;

use super::App;

impl App {
    /// Select another session client-side. There is no daemon session switch;
    /// subsequent RPCs use explicit session ids, and the daemon publishes
    /// per-session state when the client subscribes to that session.
    pub(crate) async fn select_session(&mut self, id: String) -> Result<()> {
        // An explicit selection supersedes any pending deferred fresh attach
        // (issue #46): the user picked a real session, so nothing is created
        // on the next message.
        self.pending_fresh_attach = false;
        self.session_id = id.clone();
        self.latest.session_id = id.clone();
        self.system_line(format!("selected session {id}"));
        self.refresh_session_snapshot().await;
        Ok(())
    }

    /// Refresh the nested session snapshot for the currently selected session.
    /// The TUI uses it to render session lineage and collapsed graph nodes;
    /// failures are non-fatal (the legacy `WireStatus` view remains usable).
    pub(super) async fn refresh_session_snapshot(&mut self) {
        match self.client.get_snapshot_for_session(&self.session_id).await {
            Ok(snapshot) => {
                self.session_snapshot = Some(
                    theway_transport::proto::wire_session_snapshot_from_proto(&snapshot),
                );
            }
            Err(_) => {
                self.session_snapshot = None;
            }
        }
    }

    /// Issue #46: create + select the deferred fresh session. Called right
    /// before the first submitted message reaches the daemon (reused-daemon
    /// fresh attach, issue #56) — the session file is only created when the
    /// TUI actually sends something, so an idle TUI leaves no empty
    /// conversation behind. Idempotent: no-op once the flag is cleared.
    pub(crate) async fn ensure_fresh_session(&mut self) -> Result<()> {
        if !self.pending_fresh_attach {
            return Ok(());
        }
        // Clear BEFORE select_session (which also clears it); the message
        // right after this call must not re-trigger creation.
        self.pending_fresh_attach = false;
        let summary = self
            .client
            .create_session_with_metadata(None, None, Default::default())
            .await?;
        let id = summary.session_id;
        self.select_session(id.clone()).await?;
        self.system_line(format!("new session {id}"));
        Ok(())
    }

    /// Issue #47: on exit, delete the session the SPAWNED daemon created at
    /// startup when no message ever reached it — an idle TUI must not leave
    /// an empty conversation behind. Best-effort: the daemon may already be
    /// gone, or the session may be protected (running graphs).
    pub(crate) async fn reap_empty_auto_session(&mut self) {
        let Some(id) = self.auto_session.clone() else {
            return;
        };
        if self.messaged_sessions.contains(&id) {
            return;
        }
        match self.client.delete_session(&id).await {
            Ok(running) if running.is_empty() => {
                tracing::debug!("reaped empty startup session {id}");
            }
            Ok(running) => {
                tracing::debug!("startup session {id} kept: active graphs {running:?}");
            }
            Err(error) => {
                tracing::debug!("startup session {id} reap skipped: {error}");
            }
        }
    }

    /// Resolve the pending daemon control-plane prompt through the `approve`
    /// RPC (the snapshot clears the card on the next frame).
    pub(super) fn resolve_control_plane_prompt(&mut self, approve: bool) {
        let Some(prompt) = self.control_plane_prompt.take() else {
            return;
        };
        self.system_line(format!(
            "permission {}: {}",
            if approve { "allowed" } else { "denied" },
            prompt.tool_name
        ));
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            let mut client = client;
            if let Err(e) = client.approve_for_session(&session_id, approve).await {
                eprintln!("approve: {e}");
            }
        });
    }
}
