//! Web-relay import + shared approval surface (`App` methods split out of `ui/mod.rs`).
//!
//! Covers the `/web-connect` relay lifecycle, remote prompt injection, feed-update
//! application, and the control-plane-prompt confirm surface (raise + resolve).

use std::path::PathBuf;

use crate::commands;
use crate::control_plane_prompt::UiControlPlanePrompt;

use super::kernel::TurnState;
use super::relay;
use super::render_utils::{safe_control_prompt_label, safe_control_prompt_text};
use super::{App, CONTROL_PROMPT_TEXT_WIDTH, FeedUpdate, PendingImportActivation};

#[cfg(feature = "tui")]
use super::feed::Level;

const IMPORT_ACTIVATION_PROMPT_ID: &str = "session-import-activation";

impl App {
    /// Handle `/web-connect` family outcomes. Shared by the TUI and web event loops.
    pub(super) async fn handle_web_relay(&mut self, action: commands::WebRelayAction) {
        use commands::WebRelayAction;
        match action {
            WebRelayAction::Connect => {
                if let Some(active) = &self.relay {
                    self.system_line(format!("web relay already active: {}", active.url));
                    return;
                }
                let base = match crate::config::relay_base_url().await {
                    Ok(base) => base,
                    Err(e) => {
                        self.error_line(format!("web-connect: {e}"));
                        return;
                    }
                };
                match relay::start(
                    &base,
                    self.relay_prompt_tx.clone(),
                    self.relay_abort_tx.clone(),
                    self.relay_resolve_tx.clone(),
                    self.relay_model_tx.clone(),
                ) {
                    Ok(handle) => {
                        self.system_line(format!("web relay: {}", handle.url));
                        self.system_line(
                            "warning: anyone with this URL can watch the full conversation, \
                             send prompts, AND approve permission requests until /web-disconnect",
                        );
                        #[cfg(feature = "tui")]
                        if self.relay_qr_in_feed {
                            match relay::qr_lines(&handle.url) {
                                Ok(lines) => {
                                    self.feed.push_plain_untimed("", Level::Qr);
                                    for line in lines {
                                        self.feed.push_plain_untimed(line, Level::Qr);
                                    }
                                    self.feed.push_plain_untimed(
                                        "scan with your phone to open the session",
                                        Level::System,
                                    );
                                }
                                Err(e) => self.system_line(format!("qr render skipped: {e}")),
                            }
                        }
                        self.relay = Some(handle);
                        self.push_relay_snapshot();
                    }
                    Err(e) => self.error_line(format!("web-connect: {e}")),
                }
            }
            WebRelayAction::Status => match &self.relay {
                Some(active) => self.system_line(active.status_line()),
                None => self.system_line("web relay is off — start one with /web-connect"),
            },
            WebRelayAction::Disconnect => match self.relay.take() {
                Some(active) => {
                    active.shutdown();
                    self.system_line("web relay disconnected; the session URL is now invalid");
                }
                None => self.system_line("web relay is not active"),
            },
        }
    }

    /// Queue the current state to the relay, if one is connected. Cheap when off; the
    /// relay task debounces actual sends.
    pub(super) fn push_relay_snapshot(&self) {
        if let Some(active) = &self.relay {
            active.push_snapshot(self.web_snapshot());
        }
    }

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
        let (responder, _discarded) = tokio::sync::oneshot::channel();
        let prompt = crate::control_plane_prompt::UiControlPlanePrompt {
            request: theway_core::ControlPlanePromptRequest {
                tool_call_id: IMPORT_ACTIVATION_PROMPT_ID.to_string(),
                tool_name: "SessionImport".to_string(),
                args_hash: String::new(),
                label,
                payload: serde_json::json!({
                    "triggers": trigger_ids,
                    "cron_jobs": cron_ids,
                }),
                reason: "re-enable automation imported from a session archive".to_string(),
            },
            responder,
        };
        self.pending_import_activation = Some(PendingImportActivation {
            session_path,
            trigger_ids,
            cron_ids,
        });
        self.show_control_plane_prompt(prompt);
    }

    /// Resolve a pending control-plane prompt from the relay — first-class, identical
    /// to a local confirmation (owner decision 2026-06-11).
    pub(super) fn resolve_from_relay(&mut self, approve: bool) {
        if self.control_plane_prompt.is_none() {
            return;
        }
        let decision = if approve {
            theway_core::ControlPlanePromptDecision::Allow
        } else {
            theway_core::ControlPlanePromptDecision::Deny {
                reason: Some("denied via web relay".into()),
            }
        };
        self.resolve_control_plane_prompt(decision);
    }

    /// Inject a prompt that arrived over the relay through the same path as local
    /// input. Remote slash commands are refused — the capability URL grants prompting,
    /// not REPL control.
    pub(super) fn submit_remote_text(&mut self, text: String, turn: &mut TurnState) {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with('/') {
            self.system_line("[web] remote slash command refused");
            return;
        }
        let display = format!("[web] {trimmed}");
        self.follow = true;
        if turn.fut.is_some() {
            self.queue_user_prompt(display, trimmed, Vec::new());
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(trimmed, Vec::new(), turn);
        }
    }

    pub(super) fn apply_feed_update(&mut self, update: FeedUpdate) {
        match update {
            FeedUpdate::TriggerPollStatus(status) => {
                self.latest_trigger_poll = Some(status);
            }
            other => self.feed.apply(other),
        }
    }

    pub(super) fn show_control_plane_prompt(&mut self, prompt: UiControlPlanePrompt) {
        let label = safe_control_prompt_label(&prompt.request.label);
        self.control_plane_prompt = Some(prompt);
        self.system_line(format!("approval required: {label}"));
        self.follow = true;
    }

    pub(super) fn resolve_control_plane_prompt(
        &mut self,
        decision: theway_core::ControlPlanePromptDecision,
    ) {
        let Some(prompt) = self.control_plane_prompt.take() else {
            return;
        };
        let label = safe_control_prompt_label(&prompt.request.label);
        let message = match &decision {
            theway_core::ControlPlanePromptDecision::Allow => {
                format!("approved control-plane action: {label}")
            }
            theway_core::ControlPlanePromptDecision::Deny { reason } => {
                let reason = reason.as_deref().unwrap_or("denied by user");
                let reason = safe_control_prompt_text(reason, CONTROL_PROMPT_TEXT_WIDTH);
                format!("denied control-plane action: {label} ({reason})")
            }
            theway_core::ControlPlanePromptDecision::Timeout => {
                format!("control-plane action timed out: {label}")
            }
        };
        let is_import_activation = prompt.request.tool_call_id == IMPORT_ACTIVATION_PROMPT_ID;
        let approved = matches!(decision, theway_core::ControlPlanePromptDecision::Allow);
        prompt.resolve(decision);
        self.system_line(message);
        if is_import_activation && let Some(pending) = self.pending_import_activation.take() {
            if approved {
                match crate::session_archive::activate_imported(
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
        }
    }
}
