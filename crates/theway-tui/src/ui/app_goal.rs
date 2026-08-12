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

    /// Resolve the pending control-plane prompt: import-activation cards resolve
    /// locally, daemon cards go through the `approve` RPC (the snapshot clears
    /// the card on the next frame).
    pub(super) fn resolve_control_plane_prompt(&mut self, approve: bool) {
        if let Some(pending) = self.pending_import_activation.take() {
            self.control_plane_prompt = None;
            if approve {
                match theway::session_archive::activate_imported(
                    &pending.session_path,
                    &pending.trigger_ids,
                    &pending.cron_ids,
                ) {
                    Ok((triggers, cron)) => self.system_line(format!(
                        "activated imported automation: {triggers} trigger(s), {cron} cron job(s) re-enabled"
                    )),
                    Err(e) => self.error_line(format!("activate imported automation: {e}")),
                }
            } else {
                self.system_line(
                    "imported automation stays disabled; enable later via /triggers enable and /cron enable",
                );
            }
            return;
        }
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
