impl TurnHost {
    fn wire_snapshot(&mut self) -> WireStatus {
        let model = current_model_label(self.kernel.harness());
        let context_window = context_window_for(&model);
        // Last-turn usage (not session-cumulative): the last assistant message's
        // usage, so the TUI's ctx% divides one turn's token count by the context
        // window instead of growing past it forever (issue #38).
        let usage =
            last_turn_usage(&self.kernel.harness().agent().state().messages).unwrap_or_default();
        // Authoritative snapshots always carry the full transcript. The cache
        // consumes the event-driven dirty index so clean frames do not scan
        // historical blocks; per-client tail slicing belongs to gRPC.
        let dirty_start = self.feed_dirty_start();
        self.plain_lines_cache
            .update_from_dirty(&self.feed, 100, dirty_start);
        let feed_lines = self.plain_lines_cache.rows().to_vec();
        let feed_blocks = self.feed.wire_blocks();
        let (feed_blocks_base, feed_block_patches) = self.take_feed_block_patches(&feed_blocks);
        WireStatus {
            session_id: self.session_id.clone(),
            model,
            model_catalog: self.model_catalog.clone(),
            cwd: self.cwd.display().to_string(),
            busy: self.busy,
            queued_count: self.queued_turns.len(),
            latest_trigger_poll: self.latest_trigger_poll.clone(),
            goal: self.latest_goal.as_ref().map(|goal| WireGoalSnapshot {
                condition: bug_report::redact(&goal.condition),
                status: goal.status.as_str().to_string(),
                iterations: goal.iterations,
                last_reason: goal.last_reason.as_deref().map(bug_report::redact),
            }),
            control_plane_prompt: self
                .control_plane_prompt
                .as_ref()
                .map(|prompt| wire_control_plane_prompt_snapshot(&prompt.request)),
            sidebar: self.wire_sidebar_snapshot(),
            feed_blocks,
            feed_blocks_base,
            feed_block_patches,
            feed_lines,
            feed_lines_base: 0,
            dags: self
                .dag_engine
                .list_runs()
                .iter()
                .filter(|run| run.session_id.as_deref() == Some(self.session_id.as_str()))
                .map(dag_run_snapshot)
                .collect(),
            subagents: self
                .subagent_registry
                .list()
                .iter()
                .filter(|job| job.session_id.as_deref() == Some(self.session_id.as_str()))
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
            tui_max_feed_lines: self.tui_max_feed_lines,
        }
    }

    fn feed_dirty_start(&self) -> Option<usize> {
        let block_count = self.feed.blocks().len();
        if block_count < self.block_versions.len() {
            return Some(0);
        }
        let appended =
            (block_count > self.block_versions.len()).then_some(self.block_versions.len());
        match (self.dirty_blocks.first().copied(), appended) {
            (Some(dirty), Some(appended)) => Some(dirty.min(appended)),
            (Some(dirty), None) => Some(dirty),
            (None, appended) => appended,
        }
    }

    fn take_feed_block_patches(
        &mut self,
        wire_blocks: &[theway_transport::feed::WireFeedBlock],
    ) -> (u64, Vec<WireFeedBlockPatch>) {
        use theway_transport::feed::block_fingerprint;

        let blocks = self.feed.blocks();
        if blocks.len() < self.block_versions.len() {
            self.block_versions = blocks.iter().map(block_fingerprint).collect();
            self.dirty_blocks.clear();
            return (0, Vec::new());
        }

        let base = self.block_versions.len();
        let mut dirty = std::mem::take(&mut self.dirty_blocks);
        dirty.extend(base..blocks.len());
        let mut patches = Vec::new();
        for index in dirty {
            let Some(block) = blocks.get(index) else {
                continue;
            };
            let fingerprint = block_fingerprint(block);
            if index < self.block_versions.len() {
                if self.block_versions[index] == fingerprint {
                    continue;
                }
                self.block_versions[index] = fingerprint;
            } else if index == self.block_versions.len() {
                self.block_versions.push(fingerprint);
            } else {
                continue;
            }
            patches.push(WireFeedBlockPatch {
                index: index as u64,
                block: wire_blocks[index].clone(),
            });
        }
        (base as u64, patches)
    }

    fn clear_feed(&mut self) {
        self.feed.clear();
        self.block_versions.clear();
        self.dirty_blocks.clear();
    }

    fn wire_sidebar_snapshot(&self) -> WireSidebarSnapshot {
        const ITEM_LIMIT: usize = 8;

        let skills = self.kernel.harness().skills();
        let disabled = skills
            .iter()
            .filter(|skill| skill.disable_model_invocation)
            .count();
        let enabled = skills.len().saturating_sub(disabled);
        let source_count = |source| skills.iter().filter(|skill| skill.source == source).count();

        let rules = triggers::global_registry().list();
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

        let cron_jobs = triggers::global_cron_registry().list();
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
                servers: self.panel_status.mcp_servers,
                tools: self.panel_status.mcp_tools,
                notification_hooks: self.panel_status.mcp_notification_hooks,
                server_names: self.panel_status.mcp_server_names.clone(),
                tool_names: self.panel_status.mcp_tool_names.clone(),
            },
            tools: WireToolsSnapshot {
                total: self.panel_status.tool_names.len(),
                names: self.panel_status.tool_names.clone(),
            },
            hooks: self.panel_status.hook_points.clone(),
            runtime: self.panel_status.trigger_features.clone(),
            // File commands join the snapshot so the TUI popup lists them
            // and a `/reload` republish refreshes them (issue #37).
            commands: self.registry.file_command_names(),
            // Reload epoch (issue #50): clients cache this and re-read local
            // resources (theme.toml) when the `reload` tool bumps it.
            runtime_revision: self.reload_runtime.revision.load(Ordering::SeqCst),
        }
    }

    async fn publish_snapshot(
        &mut self,
        latest: &Arc<Mutex<WireStatus>>,
        snapshots: &broadcast::Sender<WireStatus>,
    ) {
        self.sync_current_session_state();
        let snapshot = self.wire_snapshot();
        *latest.lock() = snapshot.clone();
        let _ = snapshots.send(snapshot);
    }

    // ── turn lifecycle (mirror of the TUI's app_turns, headless) ──────────────────────
}
