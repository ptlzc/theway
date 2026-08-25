impl TurnHost {
    fn wire_snapshot(&mut self) -> WireStatus {
        use theway_transport::feed::block_fingerprint;

        let dirty_start = self.feed_dirty_start();
        self.projection.plain_lines_cache
            .update_from_dirty(&self.projection.feed, 100, dirty_start);
        self.projection.block_versions = self.projection.feed.blocks().iter().map(block_fingerprint).collect();
        self.projection.dirty_blocks.clear();
        self.wire_status(
            self.projection.feed.wire_blocks(),
            0,
            Vec::new(),
            self.projection.plain_lines_cache.rows().to_vec(),
            0,
        )
    }

    fn wire_update(&mut self) -> WireStatusUpdate {
        let dirty_start = self.feed_dirty_start();
        self.projection.plain_lines_cache
            .update_from_dirty(&self.projection.feed, 100, dirty_start);
        let feed_lines_base = self.projection.plain_lines_cache.last_rebuilt_from_row;
        let feed_lines = self.projection.plain_lines_cache.rows()[feed_lines_base..].to_vec();
        let feed_lines_len = self.projection.plain_lines_cache.rows().len();
        let (feed_blocks_base, feed_block_patches) = self.take_feed_block_patches();
        let feed_blocks_len = self.projection.feed.blocks().len();
        WireStatusUpdate::delta(
            feed_blocks_base,
            feed_block_patches,
            feed_blocks_len,
            feed_lines_base as u64,
            feed_lines,
            feed_lines_len,
        )
    }

    fn wire_status(
        &self,
        feed_blocks: Vec<theway_transport::feed::WireFeedBlock>,
        feed_blocks_base: u64,
        feed_block_patches: Vec<WireFeedBlockPatch>,
        feed_lines: Vec<String>,
        feed_lines_base: u64,
    ) -> WireStatus {
        let model = current_model_label(self.session.kernel.harness());
        let context_window = context_window_for(&model);
        // Last-turn usage (not session-cumulative): the last assistant message's
        // usage so clients can compare one turn against the context window.
        let usage =
            last_turn_usage(&self.session.kernel.harness().agent().state().messages).unwrap_or_default();
        WireStatus {
            session_id: self.session.id.clone(),
            model,
            model_catalog: self.runtime.model_catalog.clone(),
            cwd: self.runtime.cwd.display().to_string(),
            busy: self.session.busy,
            queued_count: self.session.queue.len(),
            latest_trigger_poll: self.projection.latest_trigger_poll.clone(),
            goal: self.projection.latest_goal.as_ref().map(|goal| WireGoalSnapshot {
                condition: bug_report::redact(&goal.condition),
                status: goal.status.as_str().to_string(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.as_deref().map(bug_report::redact),
            }),
            control_plane_prompt: self
                .projection
                .control_plane_prompt
                .as_ref()
                .map(|prompt| wire_control_plane_prompt_snapshot(&prompt.request)),
            sidebar: self.wire_sidebar_snapshot(),
            feed_blocks,
            feed_blocks_base,
            feed_block_patches,
            feed_lines,
            feed_lines_base,
            dags: self
                .automation
                .dag
                .list_runs()
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(self.session.id.as_str()))
                .map(dag_run_snapshot)
                .collect(),
            subagents: self
                .automation
                .subagents
                .list()
                .iter()
                .filter(|job| job.session_id.as_deref() == Some(self.session.id.as_str()))
                .map(subagent_job_snapshot)
                .collect(),
            // Last-turn token usage (input/output/cache/total from the last
            // assistant message) + the active model's context window.
            usage: WireContextUsage {
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_tokens: usage.cache_read,
                cache_write_tokens: usage.cache_write,
                total_tokens: usage.total_tokens,
                context_window,
            },
            tui_max_feed_lines: self.runtime.feed_history_limit,
            extensions: self.wire_extension_snapshot(),
        }
    }

    fn feed_dirty_start(&self) -> Option<usize> {
        let block_count = self.projection.feed.blocks().len();
        if block_count < self.projection.block_versions.len() {
            return Some(0);
        }
        let appended =
            (block_count > self.projection.block_versions.len()).then_some(self.projection.block_versions.len());
        match (self.projection.dirty_blocks.first().copied(), appended) {
            (Some(dirty), Some(appended)) => Some(dirty.min(appended)),
            (Some(dirty), None) => Some(dirty),
            (None, appended) => appended,
        }
    }

    fn take_feed_block_patches(&mut self) -> (u64, Vec<WireFeedBlockPatch>) {
        use theway_transport::feed::block_fingerprint;

        let blocks = self.projection.feed.blocks();
        if blocks.len() < self.projection.block_versions.len() {
            self.projection.block_versions = blocks.iter().map(block_fingerprint).collect();
            self.projection.dirty_blocks.clear();
            return (0, Vec::new());
        }

        let base = self.projection.block_versions.len();
        let mut dirty = std::mem::take(&mut self.projection.dirty_blocks);
        dirty.extend(base..blocks.len());
        let mut patches = Vec::new();
        for index in dirty {
            let Some(block) = blocks.get(index) else {
                continue;
            };
            let fingerprint = block_fingerprint(block);
            if index < self.projection.block_versions.len() {
                if self.projection.block_versions[index] == fingerprint {
                    continue;
                }
                self.projection.block_versions[index] = fingerprint;
            } else if index == self.projection.block_versions.len() {
                self.projection.block_versions.push(fingerprint);
            } else {
                continue;
            }
            let Some(wire_block) = self.projection.feed.wire_block(index) else {
                continue;
            };
            patches.push(WireFeedBlockPatch {
                index: index as u64,
                block: wire_block,
            });
        }
        (base as u64, patches)
    }

    fn clear_feed(&mut self) {
        self.projection.feed.clear();
        self.projection.block_versions.clear();
        self.projection.dirty_blocks.clear();
    }

    fn wire_sidebar_snapshot(&self) -> WireSidebarSnapshot {
        const ITEM_LIMIT: usize = 8;

        let skills = self.session.kernel.harness().skills();
        let disabled = skills
            .iter()
            .filter(|skill| skill.disable_model_invocation)
            .count();
        let enabled = skills.len().saturating_sub(disabled);
        let source_count = |source| skills.iter().filter(|skill| skill.source == source).count();

        let rules = self.automation.services.dynamic_triggers.list();
        let trigger_enabled = rules.iter().filter(|rule| rule.enabled).count();
        let trigger_rules = rules
            .iter()
            .take(ITEM_LIMIT)
            .map(|rule| WireTriggerRuleSnapshot {
                id: feed::truncate_chars(&rule.id, 18),
                full_id: rule.id.clone(),
                enabled: rule.enabled,
                mode: if rule.fire_once { "once" } else { "repeat" }.to_string(),
                condition: wire_preview(&rule.condition),
                action: wire_preview(&rule.action),
            })
            .collect::<Vec<_>>();

        let cron_jobs = self.automation.services.cron.list();
        let cron_enabled = cron_jobs.iter().filter(|job| job.enabled).count();
        let cron_job_rows = cron_jobs
            .iter()
            .take(ITEM_LIMIT)
            .map(|job| WireCronJobSnapshot {
                id: feed::truncate_chars(&job.id, 18),
                enabled: job.enabled,
                schedule: job.schedule.clone(),
                action: wire_preview(&job.action),
                skipped_overlap_count: job.skipped_overlap_count,
                last_error: job.last_error.as_deref().map(wire_preview),
            })
            .collect::<Vec<_>>();

        WireSidebarSnapshot {
            inbox_new: theway_transport::inbox::new_count(
                &theway_transport::inbox::default_inbox_path(),
            ),
            skills: WireSkillsSnapshot {
                total: skills.len(),
                enabled,
                disabled,
                builtin: source_count(SkillSource::Builtin),
                user: source_count(SkillSource::User),
                project: source_count(SkillSource::Project),
                items: skills
                    .iter()
                    .map(|skill| WireSkillSnapshot {
                        name: skill.name.clone(),
                        source: skill.source.label().to_string(),
                        file_path: skill.file_path.clone(),
                        enabled: !skill.disable_model_invocation,
                    })
                    .collect(),
            },
            triggers: WireTriggersSnapshot {
                total: rules.len(),
                enabled: trigger_enabled,
                disabled: rules.len().saturating_sub(trigger_enabled),
                rules: trigger_rules,
            },
            cron: WireCronSnapshot {
                total: cron_jobs.len(),
                enabled: cron_enabled,
                disabled: cron_jobs.len().saturating_sub(cron_enabled),
                jobs: cron_job_rows,
            },
            mcp: WireMcpSnapshot {
                servers: self.projection.capabilities.mcp_servers,
                tools: self.projection.capabilities.mcp_tools,
                notification_hooks: self.projection.capabilities.mcp_notification_hooks,
                server_names: self.projection.capabilities.mcp_server_names.clone(),
                tool_names: self.projection.capabilities.mcp_tool_names.clone(),
            },
            tools: WireToolsSnapshot {
                total: self.projection.capabilities.tool_names.len(),
                names: self.projection.capabilities.tool_names.clone(),
            },
            hooks: self.projection.capabilities.hook_points.clone(),
            runtime: self.projection.capabilities.trigger_features.clone(),
            // File commands join the snapshot; `/reload` republishes the list.
            commands: self.runtime.registry.file_command_names(),
            // Reload epoch (issue #50): clients cache this and re-read local
            // resources (theme.toml) when the `reload` tool bumps it.
            runtime_revision: self.automation.reload.revision.load(Ordering::SeqCst),
        }
    }

    async fn publish_snapshot(
        &mut self,
        latest: &Arc<Mutex<WireStatus>>,
        snapshots: &broadcast::Sender<WireStatusUpdate>,
        metadata_dirty: bool,
    ) {
        self.sync_current_session_state();
        if metadata_dirty {
            let snapshot = self.wire_snapshot();
            *latest.lock() = snapshot.clone();
            let _ = snapshots.send(WireStatusUpdate::full(snapshot));
            return;
        }
        let update = self.wire_update();
        if update.apply_to(&mut latest.lock()) {
            let _ = snapshots.send(update);
        } else {
            let snapshot = self.wire_snapshot();
            *latest.lock() = snapshot.clone();
            let _ = snapshots.send(WireStatusUpdate::full(snapshot));
        }
    }

    async fn publish_current_snapshot(&mut self) {
        let Some(latest) = self.runtime.latest.clone() else {
            return;
        };
        let Some(snapshot_tx) = self.runtime.snapshot_tx.clone() else {
            return;
        };
        self.publish_snapshot(&latest, &snapshot_tx, true).await;
    }

    // ── turn lifecycle ────────────────────────────────────────────────────────────────
}
