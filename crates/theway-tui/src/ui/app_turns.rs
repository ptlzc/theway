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
use ratatui::backend::CrosstermBackend;

use theway_transport::commands;
use theway::images;
use theway::mentions;

use super::App;
use super::render_utils::{enter_tui, leave_tui};
use theway::session_archive;

impl App {
    // ── submit / dispatch ───────────────────────────────────────────────────────────────

    pub(super) async fn submit(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        let text = self.input_text();
        let trimmed = text.trim().to_string();
        let has_pending_images =
            !self.pending_images.is_empty() || !self.pending_pasted_images.is_empty();
        if trimmed.is_empty() && !has_pending_images {
            return Ok(());
        }
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
        match self.client.send_message(prompt_text, images, false).await {
            Ok(true) => {}
            Ok(false) => self.error_line("daemon rejected the message"),
            Err(e) => self.error_line(e.to_string()),
        }
        Ok(())
    }

    pub(super) async fn dispatch_slash(
        &mut self,
        input: &str,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
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
                "theway client · send messages to the thewayd daemon · local: /login /quit /clear /session export|import · everything else forwards to the daemon (/model /goal /triggers /cron …)",
            ),
            "/login" => self.login(args, terminal).await,
            "/session" => self.local_session_command(args).await,
            _ => {
                // Forward to the daemon: it dispatches the full slash registry
                // (model/goal/triggers/cron/skills/…) and publishes the result.
                match self
                    .client
                    .send_message(trimmed.to_string(), vec![], false)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => self.error_line("daemon rejected the command"),
                    Err(e) => self.error_line(e.to_string()),
                }
            }
        }
    }

    /// Local-only `/session` surface: export/import operate on the local SQLite
    /// repo (same machine, shared sessions) without touching the daemon.
    async fn local_session_command(&mut self, args: &str) {
        let mut parts = args.split_whitespace();
        match parts.next() {
            Some("export") => {
                let output = parts.next();
                let exclude_triggers = parts.any(|p| p == "--exclude-triggers");
                let output_path = match output {
                    Some(path) => std::path::PathBuf::from(path),
                    None => session_archive::default_export_path(&self.cwd, &self.session_id),
                };
                let output_path = if output_path.is_absolute() {
                    output_path
                } else {
                    self.cwd.join(output_path)
                };
                self.system_line(
                    "warning: .theway-session archives include transcript and tool history. They do not include separate auth stores, provider credentials, OAuth tokens, or MCP config.",
                );
                let result = async {
                    let Some(path) = theway::session::find_path_by_id(&self.session_repo, &self.session_id).await? else {
                        anyhow::bail!("current session {} not found in repo", self.session_id);
                    };
                    let session = self.session_repo.open(&path).await?;
                    session_archive::export_session(&session, &output_path, exclude_triggers).await
                }
                .await;
                match result {
                    Ok(summary) => self.system_line(format!(
                        "exported session archive: {} (entries={})",
                        summary.output_path.display(),
                        summary.entry_count
                    )),
                    Err(e) => self.error_line(format!("session export failed: {e}")),
                }
            }
            Some("import") => {
                let Some(path) = parts.next() else {
                    self.error_line("usage: /session import <path>");
                    return;
                };
                let archive_path = if std::path::Path::new(path).is_absolute() {
                    std::path::PathBuf::from(path)
                } else {
                    self.cwd.join(path)
                };
                self.system_line(
                    "warning: .theway-session archives include transcript and tool history. They do not include separate auth stores, provider credentials, OAuth tokens, or MCP config.",
                );
                match session_archive::import_session(
                    &self.session_repo,
                    &archive_path,
                    &self.cwd,
                    session_archive::ActivateTriggers::Off,
                )
                .await
                {
                    Ok(summary) => {
                        self.system_line(format!(
                            "imported session: {} (entries={})",
                            &summary.session_id[..16.min(summary.session_id.len())],
                            summary.entry_count
                        ));
                        if !summary.originally_enabled_triggers.is_empty()
                            || !summary.originally_enabled_cron.is_empty()
                        {
                            self.prompt_import_activation(
                                summary.session_path,
                                summary.originally_enabled_triggers,
                                summary.originally_enabled_cron,
                            );
                        }
                    }
                    Err(e) => self.error_line(format!("session import failed: {e}")),
                }
            }
            Some("switch") => {
                let Some(id) = parts.next() else {
                    self.error_line("usage: /session switch <id>");
                    return;
                };
                let id = id.to_string();
                match self.switch_session(id).await {
                    Ok(()) => {}
                    Err(e) => self.error_line(format!("switch session failed: {e}")),
                }
            }
            _ => self.error_line(
                "usage: /session export [path] [--exclude-triggers] | /session import <path> | /session switch <id>",
            ),
        }
    }

    pub(super) async fn login(
        &mut self,
        provider: &str,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) {
        let provider = provider.trim().to_string();
        // rpassword needs a cooked terminal with echo control, so drop out of the full-screen
        // UI for the prompt, then restore. The daemon picks the key up on its next turn via
        // the auth-store stream fn — no protocol change (design decision 6).
        leave_tui().ok();
        let result = theway::auth::prompt_for_api_key(&provider).await;
        let _ = enter_tui();
        let _ = terminal.clear();
        match result {
            Ok(token) if token.trim().is_empty() => {
                self.error_line("empty api key; login cancelled")
            }
            Ok(token) => match theway::commands::save_api_key(&provider, &token) {
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
            tokio::spawn(async move {
                let mut client = client;
                if let Err(e) = client.cancel().await {
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
