//! Turn lifecycle (`App` methods split out of `ui/mod.rs`).
//!
//! Client mode: the daemon owns the turn loop. Submit maps to
//! `send_message`, Ctrl-C to `cancel`, the control-plane card to `approve`,
//! the model picker to `set_model`. Slash commands split into a small local
//! surface (quit/clear/help/login + session export/import over the local
//! SQLite repo) and everything else forwarded to the daemon as a message
//! (the daemon dispatches the full registry and publishes the result).

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;

use theway_transport::commands;
use theway_transport::images;
use theway_transport::mentions;

use super::App;
use super::render_utils::{enter_tui, leave_tui};

impl App {
    // ── submit / dispatch ───────────────────────────────────────────────────────────────

    pub(super) async fn submit<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> Result<()> {
        let text = self.input_text();
        let trimmed = text.trim().to_string();
        let has_pending_images =
            !self.pending_images.is_empty() || !self.pending_pasted_images.is_empty();
        if trimmed.is_empty() && !has_pending_images {
            return Ok(());
        }
        // Sending resets the dragged composer height (issue #37).
        if trimmed.starts_with('/') {
            self.clear_input();
            self.history_idx = None;
            self.last_ctrlc = None;
            self.history.append(&trimmed);
            self.follow = true;
            self.dispatch_slash(&trimmed, terminal).await;
            return Ok(());
        }

        if !self.validate_pending_image_support() {
            return Ok(());
        }

        self.clear_input();
        self.history_idx = None;
        self.last_ctrlc = None;
        if !trimmed.is_empty() {
            self.history.append(&trimmed);
        }
        self.follow = true;

        let expanded = if trimmed.is_empty() {
            String::new()
        } else {
            mentions::expand(&trimmed, &self.cwd).await.0
        };
        let prompt_text =
            commands::attach_skill_prompt(expanded, self.pending_skill.take().as_deref());

        // `--image` payloads attach to the first prompt only.
        let image_paths = std::mem::take(&mut self.pending_images);
        let mut loaded_images = if image_paths.is_empty() {
            Vec::new()
        } else {
            match images::load_all(&image_paths).await {
                Ok(imgs) => imgs,
                Err(e) => {
                    self.error_line(format!("--image: {e}"));
                    Vec::new()
                }
            }
        };
        loaded_images.append(&mut self.pending_pasted_images);
        if prompt_text.trim().is_empty() && loaded_images.is_empty() {
            return Ok(());
        }

        // The daemon pushes the user block into its own feed and publishes a
        // snapshot; the client feed follows on the next frame.
        let images = loaded_images
            .into_iter()
            .map(|image| theway_transport::wire::WirePromptImage {
                data: image.data,
                name: None,
            })
            .collect();
        match self
            .client
            .send_message_to_session(Some(&self.session_id), prompt_text, images, false)
            .await
        {
            Ok(true) => {}
            Ok(false) => self.error_line("daemon rejected the message"),
            Err(e) => self.error_line(e.to_string()),
        }
        Ok(())
    }

    pub(super) async fn dispatch_slash<B: ratatui::backend::Backend>(
        &mut self,
        input: &str,
        terminal: &mut Terminal<B>,
    ) {
        let trimmed = input.trim();
        let (command, args) = trimmed
            .split_once(char::is_whitespace)
            .map_or((trimmed, ""), |(c, a)| (c, a.trim()));
        match command {
            "/quit" | "/exit" => self.quit = true,
            "/clear" => {
                self.feed.clear();
                self.follow = true;
            }
            "/help" => self.system_line(
                "theway client · send messages to the thewayd daemon · local: /login /quit /clear /new /resume /model [provider:model-id] /session switch /status-panel · daemon: /goal /triggers /cron /session …",
            ),
            "/login" => self.login(args, terminal).await,
            "/session" if args.trim_start().starts_with("switch") => {
                self.local_session_switch(args).await;
            }
            // Issue #54: open the second-level panel-mode menu (show / hide /
            // auto). Panel visibility is TUI-local state — nothing forwards
            // to the daemon.
            "/status-panel" => {
                self.status_panel_menu = Some(0);
            }
            "/extensions" => match self.client.get_extensions().await {
                Ok(extensions) => {
                    self.latest.extensions = extensions;
                    self.extension_view = true;
                }
                Err(error) => self.error_line(format!("get extensions failed: {error}")),
            },
            "/extension-reload" => {
                let cancel_active = args.split_whitespace().any(|arg| arg == "--cancel");
                match self.client.reload_extensions(cancel_active).await {
                    Ok(result) => self.system_line(format!(
                        "extension reload: {} (revision {})",
                        result.status, result.revision
                    )),
                    Err(error) => self.error_line(format!("extension reload failed: {error}")),
                }
            }
            "/extension-trust" => self.decide_extension_trust(args).await,
            command if command.starts_with("/ext:") => {
                self.invoke_extension_command(&command[5..], args).await;
            }
            // Bare `/model` uses the same curated picker as Alt-M. A concrete
            // model spec uses the typed RPC so the confirmed snapshot can
            // persist it as the next startup default. Listing remains a
            // daemon command.
            "/model" if args.is_empty() => self.open_model_picker(),
            "/model" if commands::parse_model_spec(args).is_some() => {
                self.set_model_from_spec(args).await;
            }
            // Issue #55: bare `/fork` opens the interactive picker over the
            // current session's User messages. `/fork <n>` (non-empty args)
            // misses this guard and falls through to the daemon-forwarding
            // arm below — the daemon's numbering (1 = most recent) is the
            // picker's numbering, so the forwarded text is what the daemon
            // expects.
            "/fork" if args.is_empty() => self.open_fork_picker(),
            // Issue #56: `/resume` opens the TUI-local session picker over
            // `list_sessions` (the daemon's tree order + current id). The
            // startup `--resume` terminal picker (`resume_picker.rs`) is a
            // different mechanism and stays untouched.
            "/resume" => self.open_resume_picker().await,
            "/new" => match self
                .client
                .create_session_with_metadata(None, None, Default::default())
                .await
            {
                Ok(summary) => {
                    let id = summary.session_id;
                    // `select_session` updates the client-side session id and
                    // never returns Err; /new adds the success line on top.
                    if let Err(e) = self.select_session(id.clone()).await {
                        self.error_line(format!("select session failed: {e}"));
                    } else {
                        self.system_line(format!("new session {id}"));
                    }
                }
                Err(e) => self.error_line(format!("create session failed: {e}")),
            },
            _ => {
                // Forward to the daemon: it dispatches the full slash registry
                // (model/goal/triggers/cron/skills/…) and publishes the result.
                match self
                    .client
                    .send_message_to_session(Some(&self.session_id), trimmed.to_string(), vec![], false)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => self.error_line("daemon rejected the command"),
                    Err(e) => self.error_line(e.to_string()),
                }
            }
        }
    }

    async fn invoke_extension_command(&mut self, name: &str, args: &str) {
        if name.is_empty() {
            self.error_line("extension command name is empty");
            return;
        }
        let arguments = if args.trim().is_empty() {
            serde_json::json!({})
        } else {
            match serde_json::from_str(args) {
                Ok(arguments) => arguments,
                Err(error) => {
                    self.error_line(format!("extension command arguments must be JSON: {error}"));
                    return;
                }
            }
        };
        match self
            .client
            .invoke_extension_command(name, arguments, true)
            .await
        {
            Ok(outcome) => {
                let mut summary = format!("extension command {name}: {}", outcome.status);
                if let Some(code) = outcome.code {
                    summary.push_str(&format!(" [{code}]"));
                }
                if let Some(message) = outcome.message {
                    summary.push_str(&format!(" — {message}"));
                }
                if let Some(data) = outcome.data {
                    summary.push_str(&format!(" — {data}"));
                }
                if outcome.status == "success" {
                    self.system_line(summary);
                } else {
                    self.error_line(summary);
                }
            }
            Err(error) => self.error_line(format!("extension command failed: {error}")),
        }
    }

    async fn decide_extension_trust(&mut self, args: &str) {
        let parts = args.split_whitespace().collect::<Vec<_>>();
        let parsed = match parts.as_slice() {
            ["project", decision, permissions @ ..] => {
                Some(("project", None, *decision, permissions))
            }
            ["package", extension_id, decision, permissions @ ..] => {
                Some(("package", Some(*extension_id), *decision, permissions))
            }
            _ => None,
        };
        let Some((subject, extension_id, decision, permission_args)) = parsed else {
            self.error_line(
                "usage: /extension-trust project <trusted|denied> [permissions…] | package <id> <trusted|denied> [permissions…]",
            );
            return;
        };
        if !matches!(decision, "trusted" | "denied") {
            self.error_line("extension trust decision must be trusted or denied");
            return;
        }
        let granted_permissions = if decision == "denied" {
            Vec::new()
        } else if permission_args.is_empty() {
            self.latest
                .extensions
                .catalog
                .iter()
                .filter(|entry| {
                    subject == "project" && entry.source == "project"
                        || extension_id.is_some_and(|id| entry.extension_id == id)
                })
                .flat_map(|entry| entry.permissions.iter().cloned())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            permission_args
                .iter()
                .map(|permission| (*permission).to_string())
                .collect()
        };
        let request = theway_transport::wire::WireExtensionTrustRequest {
            subject: subject.into(),
            extension_id: extension_id.map(String::from),
            decision: decision.into(),
            granted_permissions,
        };
        match self.client.decide_extension_trust(request).await {
            Ok(result) => self.system_line(format!(
                "extension trust accepted; reload {} (revision {})",
                result.reload.status, result.reload.revision
            )),
            Err(error) => self.error_line(format!("extension trust failed: {error}")),
        }
    }

    /// Issue #55: bare `/fork` opens the interactive picker over the current
    /// session's User feed blocks (newest-first, numbers matching the
    /// daemon's `/fork <n>` numbering). The picker snapshots the list at
    /// open time; Enter forwards `/fork <n>` through the normal dispatch
    /// path. An empty feed never opens the popup — the same error the daemon
    /// reports for a session with no user messages.
    pub(super) fn open_fork_picker(&mut self) {
        let entries = super::fork_picker_entries(&self.latest.feed_blocks);
        if entries.is_empty() {
            self.error_line("no user messages to fork from");
            return;
        }
        self.fork_picker = Some(super::ForkPickerState {
            entries,
            selected: 0,
            scroll: 0,
        });
    }

    /// Issue #56: `/resume` opens the TUI-local session picker over
    /// `client.list_sessions()` — rows keep the daemon's tree order
    /// (oldest → newest), carry short id + name + busy/graph marks, and the
    /// current session row is annotated and pre-selected. An empty daemon
    /// list prints a system hint instead of opening an empty popup; a list
    /// RPC failure reports an error line. Selecting a row (Enter in
    /// `handle_resume_picker_key`) selects the session client-side via
    /// `select_session`.
    pub(super) async fn open_resume_picker(&mut self) {
        let (sessions, current_id) = match self.client.list_sessions().await {
            Ok(pair) => pair,
            Err(e) => {
                self.error_line(format!("list sessions failed: {e}"));
                return;
            }
        };
        if sessions.is_empty() {
            self.system_line("no sessions to resume");
            return;
        }
        let entries: Vec<super::ResumePickerEntry> = sessions
            .iter()
            .map(|s| super::ResumePickerEntry {
                id: s.session_id.clone(),
                id_short: crate::cli::short_id(&s.session_id),
                name: s.name.clone(),
                busy: s.busy,
                graph_count: s.graph_count,
                active_graph_count: s.active_graph_count,
                current: s.session_id == current_id,
            })
            .collect();
        let selected = entries.iter().position(|e| e.current).unwrap_or(0);
        self.resume_picker = Some(super::ResumePickerState {
            entries,
            selected,
            scroll: 0,
        });
        self.sync_resume_picker_window();
    }

    /// Local `/session switch` surface (client-side session selection).
    /// Export/import run in the daemon (`/session export|import` is forwarded
    /// to it) — the session archive operates on the repo the daemon owns.
    async fn local_session_switch(&mut self, args: &str) {
        let id = args
            .trim_start()
            .strip_prefix("switch")
            .unwrap_or(args)
            .trim();
        if id.is_empty() {
            self.error_line("usage: /session switch <id>");
            return;
        }
        match self.select_session(id.to_string()).await {
            Ok(()) => {}
            Err(e) => self.error_line(format!("select session failed: {e}")),
        }
    }

    pub(super) async fn login<B: ratatui::backend::Backend>(
        &mut self,
        provider: &str,
        terminal: &mut Terminal<B>,
    ) {
        let provider = provider.trim().to_string();
        // rpassword needs a cooked terminal with echo control, so drop out of the full-screen
        // UI for the prompt, then restore. The daemon picks the key up on its next turn via
        // the auth-store stream fn — no protocol change (design decision 6).
        leave_tui().ok();
        let result = crate::local_commands::prompt_for_api_key(&provider).await;
        let _ = enter_tui();
        let _ = terminal.clear();
        match result {
            Ok(token) if token.trim().is_empty() => {
                self.error_line("empty api key; login cancelled")
            }
            Ok(token) => match theway_transport::auth::save_api_key(&provider, &token) {
                Ok(path) => self.system_line(format!(
                    "saved api key for `{provider}` to {} — the daemon picks it up on its next turn",
                    path.display()
                )),
                Err(e) => self.error_line(e),
            },
            Err(e) => self.error_line(e.to_string()),
        }
    }

    pub(super) fn request_abort(&mut self) {
        if self.busy {
            self.system_line("aborting current turn…");
            let client = self.client.clone();
            let session_id = self.session_id.clone();
            tokio::spawn(async move {
                let mut client = client;
                if let Err(e) = client.cancel_session(&session_id).await {
                    eprintln!("cancel: {e}");
                }
            });
        }
    }

    pub(super) fn handle_ctrl_d(&mut self) -> bool {
        if self.busy {
            self.request_abort();
            true
        } else {
            false
        }
    }

    pub(super) fn on_idle_ctrlc(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_ctrlc
            .map(|t| now.duration_since(t) < Duration::from_millis(1500))
            .unwrap_or(false)
        {
            return true;
        }
        self.last_ctrlc = Some(now);
        self.system_line("press Ctrl-C again within 1.5s to exit, or type /quit");
        false
    }
}
