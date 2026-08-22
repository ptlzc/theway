impl App {
    // ── snapshot application (the daemon owns the transcript) ──────────────────────────

    /// Apply either an authoritative full snapshot or a per-stream feed
    /// patch frame, then resync every renderable status field.
    pub(super) fn apply_snapshot(&mut self, mut status: WireStatus) {
        let full_feed = status.feed_blocks_base == 0 && status.feed_block_patches.is_empty();
        if full_feed {
            self.feed.replace_blocks(&status.feed_blocks);
            for line in &self.connection_log {
                self.feed.push_plain(line, Level::System);
            }
            self.resync_pending = false;
        } else if status.feed_blocks_base == self.latest.feed_blocks.len() as u64 {
            let current_len = self.latest.feed_blocks.len();
            let mut projected_len = current_len;
            let mut appended = Vec::new();
            let valid = status.feed_block_patches.iter().all(|patch| {
                let Ok(index) = usize::try_from(patch.index) else {
                    return false;
                };
                if index > projected_len {
                    return false;
                }
                let existing = if index < current_len {
                    self.latest.feed_blocks.get(index)
                } else if index < projected_len {
                    appended.get(index - current_len).copied()
                } else {
                    None
                };
                if let Some(existing) = existing
                    && std::mem::discriminant(existing)
                        != std::mem::discriminant(&patch.block)
                {
                    return false;
                }
                if index == projected_len {
                    appended.push(&patch.block);
                    projected_len += 1;
                }
                true
            });
            if valid {
                let mut blocks = std::mem::take(&mut self.latest.feed_blocks);
                let mut render_out_of_sync = false;
                for patch in &status.feed_block_patches {
                    let index = patch.index as usize;
                    if index == blocks.len() {
                        self.feed.append_blocks(std::slice::from_ref(&patch.block));
                        blocks.push(patch.block.clone());
                    } else {
                        blocks[index] = patch.block.clone();
                        if !self.feed.replace_block(index, &patch.block) {
                            render_out_of_sync = true;
                        }
                    }
                }
                if render_out_of_sync {
                    self.feed.replace_blocks(&blocks);
                }
                status.feed_blocks = blocks;
                self.resync_pending = false;
            } else {
                status.feed_blocks = std::mem::take(&mut self.latest.feed_blocks);
                self.resync_pending = true;
            }
        } else {
            status.feed_blocks = std::mem::take(&mut self.latest.feed_blocks);
            self.resync_pending = true;
        }
        // `latest` is always an authoritative local cache, never another
        // incremental frame waiting to be applied.
        status.feed_blocks_base = 0;
        status.feed_block_patches.clear();
        self.latest = status;
        self.session_id = self.latest.session_id.clone();
        // Daemon-side reload (issue #50): the `reload` tool bumped the
        // runtime revision, so re-read the local theme file — theme.toml
        // edits land without a restart.
        let runtime_revision = self.latest.sidebar.runtime_revision;
        if runtime_revision != self.last_runtime_revision {
            self.last_runtime_revision = runtime_revision;
            self.theme = Theme::load();
        }
        let was_busy = self.busy;
        self.busy = self.latest.busy;
        if self.busy && !was_busy {
            // Fresh busy window: restart the pixel-loader elapsed timer.
            self.busy_started = Some(Instant::now());
        } else if !self.busy {
            self.busy_started = None;
        }
        self.panel_status = PanelStatus::from_sidebar(&self.latest.sidebar);
        self.model_catalog = self.latest.model_catalog.clone();
        self.persist_confirmed_model_default();
        self.control_plane_prompt = self.latest.control_plane_prompt.clone();
        self.latest_goal = self.latest.goal.clone();
        self.latest_trigger_poll = self.latest.latest_trigger_poll.clone();
        self.connected = true;
        // `follow` is deliberately NOT forced here. A scrolled-up view stays
        // pinned while the stream appends; follow is only re-enabled by an
        // explicit user action or by scrolling back to the bottom.
    }

    /// A successful SetModel RPC only means the daemon accepted the command.
    /// Persist after a snapshot confirms the requested pair in the same
    /// session, so a failed runtime application never becomes a startup
    /// default.
    fn persist_confirmed_model_default(&mut self) {
        let Some(pending) = self.pending_model_default.as_ref() else {
            return;
        };
        if pending.session_id != self.latest.session_id {
            self.pending_model_default = None;
            return;
        }
        let expected = format!(
            "{}:{}",
            pending.selection.provider, pending.selection.model
        );
        if self.latest.model != expected {
            return;
        }

        let pending = self
            .pending_model_default
            .take()
            .expect("pending model default was checked above");
        match crate::config_payload::persist_model_default(
            &self.model_config_path,
            &pending.selection,
        ) {
            Ok(()) => {
                if let Some(hint) = theway_transport::auth::model_credential_hint(
                    &pending.selection.provider,
                ) {
                    self.system_line(format!(
                        "selected {expected} and saved it as the startup default, but login is required: {hint}"
                    ));
                } else {
                    self.system_line(format!(
                        "switched to {expected}; saved as the startup default"
                    ));
                }
            }
            Err(error) => self.error_line(format!(
                "switched to {expected}, but could not save the startup default: {error}"
            )),
        }
    }

    /// Apply one stream frame. Snapshots carry full non-feed state plus either
    /// a full transcript or feed patches. `StreamEvent` carries graph-plane increments
    /// (subagent_*/node_status/run_status); the TUI has no graph panel yet —
    /// `latest.dags`/`latest.subagents` refresh via snapshots only. There is
    /// no feed event kind, so feed blocks travel in snapshots; events are
    /// ignored deliberately rather than mapped onto unrelated UI state.
    pub(super) fn apply_frame(&mut self, frame: theway_grpc::StreamFrame) {
        match frame.payload {
            Some(stream_frame::Payload::Snapshot(state)) => {
                self.apply_snapshot(wire_status(&state));
            }
            Some(stream_frame::Payload::Event(_)) | None => {}
        }
    }

    // ── main entry ──────────────────────────────────────────────────────────────────────
}
