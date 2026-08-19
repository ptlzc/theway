impl TurnHost {
    fn queue_user_prompt(&mut self, display: String, prompt: String, images: Vec<ImageContent>) {
        self.enqueue_turn(QueuedTurn::UserPrompt {
            display,
            prompt,
            images,
        });
    }

    fn enqueue_turn(&mut self, job: QueuedTurn) {
        let preview = feed::truncate_chars(job.display(), 80);
        self.queued_turns.push_back(job);
        self.system_line(format!(
            "queued next message #{}: {preview}",
            self.queued_turns.len()
        ));
    }

    fn start_next_queued_turn(&mut self, turn: &mut TurnState) -> bool {
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

    fn start_prompt_turn(
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

    fn start_user_prompt_turn(
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

    fn start_template_turn(
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

    fn start_compaction_turn(&mut self, custom: Option<String>, turn: &mut TurnState) {
        turn.fut = Some(self.kernel.compaction_turn(custom));
        turn.aborted = false;
        turn.prefix = "compaction failed: ";
        self.busy = true;
    }

    fn start_triggered_turn(&mut self, trace_id: String, turn: &mut TurnState) {
        if self.kernel.is_streaming() {
            return;
        }
        let short: String = trace_id.chars().take(8).collect();
        self.system_line(format!("running triggered turn (trace {short})"));
        turn.fut = Some(self.kernel.continue_turn());
        turn.aborted = false;
        turn.prefix = "triggered turn: ";
        self.busy = true;
    }

    fn request_abort(&mut self, turn: &mut TurnState) {
        if turn.fut.is_some() {
            turn.aborted = true;
            self.kernel.abort();
            self.system_line("aborting current turn…");
        }
    }

    async fn finish_turn(
        &mut self,
        turn: &mut TurnState,
        result: Result<Option<String>, theway_core::AgentRunError>,
    ) {
        turn.fut = None;
        self.busy = false;
        if turn.aborted {
            self.system_line("[aborted]");
        } else {
            match result {
                Ok(Some(message)) => self.system_line(message),
                Ok(None) => {}
                Err(e) => self.error_line(format!(
                    "{}{}",
                    turn.prefix,
                    user_facing_run_error(&e.to_string())
                )),
            }
        }
        turn.aborted = false;
        turn.prefix = "";
        self.refresh_goal_state().await;
        self.start_next_queued_turn(turn);
    }

    // ── state helpers ──────────────────────────────────────────────────────────────────
}
