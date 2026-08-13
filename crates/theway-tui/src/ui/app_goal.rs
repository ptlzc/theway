//! Goal + session state (`App` methods split out of `ui/mod.rs`).
//!
//! Client mode: goal state arrives in snapshots (`latest.goal`); session
//! switching and control-plane resolution are RPC calls. The import-activation
//! card is resolved locally (it writes sidecar files, no daemon round-trip).

use anyhow::Result;

use super::App;

impl App {
    /// Ask the daemon to switch to another session (aborts an in-flight turn
    /// daemon-side; the next snapshot reflects the new session).
    pub(crate) async fn switch_session(&mut self, id: String) -> Result<()> {
        match self.client.switch_session(&id).await {
            Ok(true) => {
                self.system_line(format!("switching to session {id}…"));
                Ok(())
            }
            Ok(false) => {
                self.error_line("daemon rejected the session switch");
                Ok(())
            }
            Err(e) => {
                self.error_line(format!("switch_session failed: {e}"));
                Ok(())
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
        tokio::spawn(async move {
            let mut client = client;
            if let Err(e) = client.approve(approve).await {
                eprintln!("approve: {e}");
            }
        });
    }
}
