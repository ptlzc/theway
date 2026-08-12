//! Import activation + shared confirm surface (`App` methods split out of `ui/mod.rs`).
//!
//! `/session import` brings automation that the source had enabled; the TUI
//! raises the same confirm card used for daemon control-plane prompts, but
//! resolves it **locally** (the activation writes `~/.theway` sidecar files —
//! no daemon round-trip).

use std::path::PathBuf;

use super::render_utils::safe_control_prompt_label;
use super::{App, PendingImportActivation};
use theway_transport::wire::WireControlPlanePromptSnapshot;

const IMPORT_ACTIVATION_TOOL: &str = "SessionImport";

impl App {
    /// `/session import` brought automation that the source had enabled. Raise the
    /// shared confirm surface; approval restores exactly the source enablement.
    pub(super) fn prompt_import_activation(
        &mut self,
        session_path: PathBuf,
        trigger_ids: Vec<String>,
        cron_ids: Vec<String>,
    ) {
        if self.control_plane_prompt.is_some() {
            // A real tool prompt is pending; don't fight over the surface. Leave the
            // automation disabled — /triggers enable and /cron enable still work.
            self.system_line(
                "imported automation left disabled (another approval is pending); enable via /triggers enable and /cron enable",
            );
            return;
        }
        let label = format!(
            "activate imported automation? ({} trigger(s), {} cron job(s) were enabled at the source)",
            trigger_ids.len(),
            cron_ids.len()
        );
        self.control_plane_prompt = Some(WireControlPlanePromptSnapshot {
            tool_name: IMPORT_ACTIVATION_TOOL.to_string(),
            label: safe_control_prompt_label(&label),
            reason: "re-enable automation imported from a session archive".to_string(),
            args_hash: String::new(),
            payload: serde_json::to_string_pretty(&serde_json::json!({
                "triggers": trigger_ids,
                "cron_jobs": cron_ids,
            }))
            .unwrap_or_default(),
        });
        self.pending_import_activation = Some(PendingImportActivation {
            session_path,
            trigger_ids,
            cron_ids,
        });
        self.system_line("approval required: activate imported automation?");
        self.follow = true;
    }
}
