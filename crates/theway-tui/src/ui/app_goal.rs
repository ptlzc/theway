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
        self.session_id = id.clone();
        self.latest.session_id = id.clone();
        self.system_line(format!("selected session {id}"));
        Ok(())
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
