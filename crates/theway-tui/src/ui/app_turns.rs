//! Turn lifecycle (`App` methods split out of `ui/mod.rs`).
//!
//! Submit + slash dispatch, the queued-turn FIFO, the per-kind turn starters, OAuth
//! login, and abort/exit semantics (Ctrl-C / Ctrl-D while a turn runs).

use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use theway_llm_provider::ImageContent;

use theway::commands;
use theway::commands::{CommandCtx, CommandOutcome};
use theway::images;
use theway::mentions;

use super::App;
use super::kernel::{QueuedTurn, TurnState};
use super::render_utils::queue_preview;
use super::render_utils::{enter_tui, leave_tui, prompt_display};

impl App {
    // ── submit / dispatch ───────────────────────────────────────────────────────────────

    pub(super) async fn submit(
        &mut self,
        turn: &mut TurnState,
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
            self.feed.push_user(&trimmed);
            self.dispatch_slash(&trimmed, terminal, turn).await;
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

        let display = prompt_display(&trimmed, loaded_images.len());

        if turn.fut.is_some() {
            self.queue_user_prompt(display, prompt_text, loaded_images);
        } else {
            self.feed.push_user(display);
            self.start_user_prompt_turn(prompt_text, loaded_images, turn);
        }
        Ok(())
    }

    pub(super) fn start_triggered_turn(&mut self, trace_id: String, turn: &mut TurnState) {
        // The kernel emits this only for an idle parent, but a user prompt may have started in
        // the gap; `continue_` would return AlreadyStreaming. Skip rather than error.
        if self.kernel.is_streaming() {
            return;
        }
        let short: String = trace_id.chars().take(8).collect();
        self.system_line(format!("running triggered turn (trace {short})"));
        self.follow = true;
        turn.fut = Some(self.kernel.continue_turn());
        turn.aborted = false;
        turn.prefix = "triggered turn: ";
        self.busy = true;
    }

    pub(super) fn queue_user_prompt(
        &mut self,
        display: String,
        prompt: String,
        images: Vec<ImageContent>,
    ) {
        self.enqueue_turn(QueuedTurn::UserPrompt {
            display,
            prompt,
            images,
        });
    }

    pub(super) fn enqueue_turn(&mut self, job: QueuedTurn) {
        let preview = queue_preview(job.display());
        self.queued_turns.push_back(job);
        self.system_line(format!(
            "queued next message #{}: {preview}",
            self.queued_turns.len()
        ));
    }

    pub(super) fn cancel_last_queued_turn(&mut self) {
        let Some(job) = self.queued_turns.pop_back() else {
            self.system_line("queue is empty");
            return;
        };
        let preview = queue_preview(job.display());
        self.system_line(format!("removed queued message: {preview}"));
    }

    pub(super) fn start_next_queued_turn(&mut self, turn: &mut TurnState) -> bool {
        if turn.fut.is_some() {
            return true;
        }
        let Some(job) = self.queued_turns.pop_front() else {
            return false;
        };
        let remaining = self.queued_turns.len();
        self.system_line(if remaining == 0 {
            "running queued message".to_string()
        } else {
            format!("running queued message ({remaining} still queued)")
        });
        match job {
            QueuedTurn::UserPrompt {
                display,
                prompt,
                images,
            } => {
                self.feed.push_user(display);
                self.start_user_prompt_turn(prompt, images, turn);
            }
            QueuedTurn::AgentPrompt {
                display,
                prompt,
                error_context,
            } => {
                self.feed.push_user(display);
                self.start_prompt_turn(prompt, error_context, turn);
            }
            QueuedTurn::PromptTemplate {
                display,
                name,
                vars,
            } => {
                self.feed.push_user(display);
                self.start_template_turn(name, vars, turn);
            }
            QueuedTurn::Compaction { display, custom } => {
                self.feed.push_user(display);
                self.start_compaction_turn(custom, turn);
            }
        }
        true
    }

    pub(super) async fn dispatch_slash(
        &mut self,
        input: &str,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
        turn: &mut TurnState,
    ) {
        let outcome = {
            let ctx = CommandCtx {
                harness: self.kernel.harness(),
                trigger_executor: self.kernel.trigger_executor(),
                session_id: &self.session_id,
                log_path: self.log_path.as_ref(),
                tool_count: self.tool_count,
                cwd: &self.cwd,
            };
            commands::dispatch(input, &self.registry, &ctx).await
        };
        match outcome {
            CommandOutcome::Quit => self.quit = true,
            CommandOutcome::ClearScreen => {
                self.feed.clear();
                self.follow = true;
            }
            CommandOutcome::Error(e) => self.error_line(e),
            CommandOutcome::AttachSkill { name } => {
                self.pending_skill = Some(name);
            }
            CommandOutcome::RunAgentPrompt {
                prompt,
                error_context,
            } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::AgentPrompt {
                        display: input.to_string(),
                        prompt,
                        error_context,
                    });
                } else {
                    self.start_prompt_turn(prompt, error_context, turn);
                }
            }
            CommandOutcome::RunPromptTemplate { name, vars } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::PromptTemplate {
                        display: input.to_string(),
                        name,
                        vars,
                    });
                } else {
                    self.start_template_turn(name, vars, turn);
                }
            }
            CommandOutcome::RunCompaction { custom } => {
                if turn.fut.is_some() {
                    self.enqueue_turn(QueuedTurn::Compaction {
                        display: input.to_string(),
                        custom,
                    });
                } else {
                    self.start_compaction_turn(custom, turn);
                }
            }
            CommandOutcome::WebRelay(action) => self.handle_web_relay(action).await,
            CommandOutcome::SessionImportActivation {
                session_path,
                trigger_ids,
                cron_ids,
            } => self.prompt_import_activation(session_path, trigger_ids, cron_ids),
            CommandOutcome::LoginSecret {
                provider,
                storage_key,
                recovery_command: _,
            } => {
                self.login(&provider, storage_key.as_deref(), terminal)
                    .await;
            }
            CommandOutcome::OpenModelPicker => self.open_model_picker(),
            CommandOutcome::Handled => {}
        }
        if input.trim_start().starts_with("/goal") {
            self.refresh_goal_state().await;
        }
    }

    pub(super) fn start_prompt_turn(
        &mut self,
        prompt: String,
        error_context: &'static str,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.prompt_turn(prompt));
        turn.aborted = false;
        turn.prefix = error_context;
        self.busy = true;
    }

    pub(super) fn start_user_prompt_turn(
        &mut self,
        prompt_text: String,
        loaded_images: Vec<ImageContent>,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.user_prompt_turn(prompt_text, loaded_images));
        turn.aborted = false;
        turn.prefix = "";
        self.busy = true;
    }

    pub(super) fn start_template_turn(
        &mut self,
        name: String,
        vars: serde_json::Map<String, serde_json::Value>,
        turn: &mut TurnState,
    ) {
        turn.fut = Some(self.kernel.template_turn(name, vars));
        turn.aborted = false;
        turn.prefix = "template run failed: ";
        self.busy = true;
    }

    pub(super) fn start_compaction_turn(&mut self, custom: Option<String>, turn: &mut TurnState) {
        turn.fut = Some(self.kernel.compaction_turn(custom));
        turn.aborted = false;
        turn.prefix = "compaction failed: ";
        self.busy = true;
    }

    pub(super) async fn login(
        &mut self,
        provider: &str,
        storage_key: Option<&str>,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) {
        // rpassword needs a cooked terminal with echo control, so drop out of the full-screen
        // UI for the prompt, then restore.
        leave_tui().ok();
        let result = theway::auth::prompt_for_api_key(provider).await;
        let _ = enter_tui();
        let _ = terminal.clear();
        match result {
            Ok(token) if token.trim().is_empty() => {
                self.error_line("empty api key; login cancelled")
            }
            Ok(token) => match commands::save_api_key(storage_key.unwrap_or(provider), &token) {
                Ok(path) => self.system_line(format!(
                    "saved api key for `{provider}` to {}",
                    path.display()
                )),
                Err(e) => self.error_line(e),
            },
            Err(e) => self.error_line(e.to_string()),
        }
    }

    pub(super) fn request_abort(&mut self, turn: &mut TurnState) {
        if turn.fut.is_some() {
            turn.aborted = true;
            self.kernel.abort();
            self.system_line("aborting current turn…");
        }
    }

    pub(super) fn handle_ctrl_d(&mut self, turn: &mut TurnState) -> bool {
        if turn.fut.is_some() {
            self.request_abort(turn);
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
