impl App {
    fn render_trigger_panel(&self, frame: &mut ratatui::Frame, area: Rect) {
        let lines =
            self.trigger_panel_lines(area.width.saturating_sub(2) as usize, area.height as usize);
        let panel = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::LEFT)
                .padding(Padding::left(1))
                .title(" Automation ")
                .border_style(Style::default().fg(self.theme.sidebar.fg))
                .title_style(Style::default().fg(self.theme.sidebar.heading)),
        );
        frame.render_widget(panel, area);
    }

    /// Resolve the side panel's rendered width from the visibility mode
    /// (issue #54): `None` hides the panel.
    fn side_panel_width(&self, content_width: u16) -> Option<u16> {
        resolve_side_panel_width(
            self.side_panel_mode,
            self.should_show_side_panel(),
            content_width,
        )
    }

    fn should_show_side_panel(&self) -> bool {
        let sidebar = &self.latest.sidebar;
        !sidebar.skills.items.is_empty()
            || !sidebar.triggers.rules.is_empty()
            || !sidebar.cron.jobs.is_empty()
            || self.latest.latest_trigger_poll.is_some()
            || self.latest.goal.is_some()
            || sidebar.mcp.servers > 0
            || sidebar.mcp.notification_hooks > 0
            || !self.latest.extensions.catalog.is_empty()
            || self.latest.extensions.reload_pending
            || self.latest.extensions.contributions.iter().any(|contribution| {
                matches!(contribution.kind.as_str(), "status_item" | "notification")
            })
    }

    fn trigger_panel_lines(&self, width: usize, height: usize) -> Vec<Line<'static>> {
        let width = width.max(1);
        let s = self.theme.sidebar;
        let sidebar = &self.latest.sidebar;
        let skills = &sidebar.skills.items;
        let rules = &sidebar.triggers.rules;
        let cron_jobs = &sidebar.cron.jobs;

        let mut lines = Vec::new();
        if !self.latest.extensions.catalog.is_empty()
            || self.latest.extensions.reload_pending
            || !self.latest.extensions.contributions.is_empty()
        {
            lines.push(panel_line("Extensions".to_string(), s.section, width));
            let effective = self
                .latest
                .extensions
                .catalog
                .iter()
                .filter(|entry| entry.status == "effective")
                .count();
            let unavailable = self
                .latest
                .extensions
                .catalog
                .len()
                .saturating_sub(effective);
            lines.push(panel_line(
                format!(
                    "active {effective} · unavailable {unavailable}{}",
                    if self.latest.extensions.reload_pending {
                        " · reload pending"
                    } else {
                        ""
                    }
                ),
                if self.latest.extensions.reload_pending || unavailable > 0 {
                    s.warn
                } else {
                    s.badge
                },
                width,
            ));
            for contribution in &self.latest.extensions.contributions {
                let summary = match contribution.kind.as_str() {
                    "status_item" => Some(format!(
                        "{}: {}",
                        contribution
                            .payload
                            .get("label")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("status"),
                        contribution
                            .payload
                            .get("value")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                    )),
                    "notification" => contribution
                        .payload
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .map(|title| format!("notice: {title}")),
                    _ => None,
                };
                if let Some(summary) = summary {
                    lines.push(panel_line(summary, s.muted, width));
                }
            }
            lines.push(Line::raw(""));
        }
        lines.push(panel_line("Skills".to_string(), s.section, width));
        if skills.is_empty() {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            let disabled = skills.iter().filter(|skill| !skill.enabled).count();
            let enabled = skills.len().saturating_sub(disabled);
            lines.push(panel_line(
                format!("enabled {enabled} · disabled {disabled}"),
                if disabled == 0 {
                    s.badge
                } else {
                    s.warn
                },
                width,
            ));
            let source_count =
                |source| skills.iter().filter(|skill| skill.source == source).count();
            lines.push(panel_line(
                format!(
                    "builtin {} · user {} · project {}",
                    source_count("builtin"),
                    source_count("user"),
                    source_count("project")
                ),
                s.muted,
                width,
            ));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Triggers".to_string(), s.section, width));
        if rules.is_empty() {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            for rule in rules.iter().take(TRIGGER_PANEL_RULE_LIMIT) {
                let state_flag = if rule.enabled { "enabled" } else { "disabled" };
                let color = if rule.enabled {
                    s.badge
                } else {
                    s.muted
                };
                lines.push(panel_line(
                    format!(
                        "{id} [{state_flag}, {mode}]",
                        id = rule.id,
                        mode = rule.mode
                    ),
                    color,
                    width,
                ));
                lines.push(panel_line(
                    format!("  when {}", panel_rule_preview(&rule.condition, width)),
                    s.muted,
                    width,
                ));
                lines.push(panel_line(
                    format!("  do   {}", panel_rule_preview(&rule.action, width)),
                    s.muted,
                    width,
                ));
            }
            if rules.len() > TRIGGER_PANEL_RULE_LIMIT {
                lines.push(panel_line(
                    format!("… {} more", rules.len() - TRIGGER_PANEL_RULE_LIMIT),
                    s.muted,
                    width,
                ));
            }
        }

        if let Some(status) = &self.latest.latest_trigger_poll {
            lines.push(Line::raw(""));
            lines.push(panel_line("Polling".to_string(), s.section, width));
            lines.push(panel_line(
                format!("{} · no match", status.checked_at),
                s.warn,
                width,
            ));
            lines.push(panel_line(
                format!(
                    "{} / {}",
                    panel_rule_preview(&status.source_label, width),
                    panel_rule_preview(&status.event_label, width)
                ),
                s.muted,
                width,
            ));
            lines.push(panel_line(
                format!("trace {}", panel_rule_preview(&status.trace_id, width)),
                s.muted,
                width,
            ));
            lines.push(panel_line(
                format!("  {}", panel_rule_preview(&status.summary, width)),
                s.muted,
                width,
            ));
        }

        if let Some(goal) = &self.latest.goal {
            lines.push(Line::raw(""));
            lines.push(panel_line("Goal".to_string(), s.section, width));
            let color = match goal.status.as_str() {
                "pursuing" => s.warn,
                "achieved" => s.badge,
                "paused" | "budget_limited" | "cleared" => s.muted,
                _ => s.muted,
            };
            lines.push(panel_line(goal.status.clone(), color, width));
            lines.push(panel_line(
                panel_rule_preview(&goal.condition, width),
                s.muted,
                width,
            ));
            if goal.iterations > 0 {
                lines.push(panel_line(
                    format!("checks {}", goal.iterations),
                    s.muted,
                    width,
                ));
            }
            if let Some(reason) = goal.last_reason.as_deref() {
                lines.push(panel_line(
                    format!("  {}", panel_rule_preview(reason, width)),
                    s.muted,
                    width,
                ));
            }
        }

        lines.push(Line::raw(""));
        if sidebar.inbox_new > 0 {
            lines.push(panel_line(
                format!("Inbox  {} new — /inbox", sidebar.inbox_new),
                s.warn,
                width,
            ));
            lines.push(panel_line(String::new(), Color::Reset, width));
        }
        lines.push(panel_line("Cron (session)".to_string(), s.section, width));
        if cron_jobs.is_empty() {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            let enabled = cron_jobs.iter().filter(|job| job.enabled).count();
            let disabled = cron_jobs.len().saturating_sub(enabled);
            lines.push(panel_line(
                format!("enabled {enabled} · disabled {disabled}"),
                if disabled == 0 {
                    s.badge
                } else {
                    s.warn
                },
                width,
            ));
            for job in cron_jobs.iter().take(TRIGGER_PANEL_RULE_LIMIT) {
                let state_flag = if job.enabled { "enabled" } else { "disabled" };
                let color = if job.enabled {
                    s.badge
                } else {
                    s.muted
                };
                lines.push(panel_line(
                    format!(
                        "{id} [{state_flag}] {schedule}",
                        id = job.id,
                        schedule = job.schedule
                    ),
                    color,
                    width,
                ));
                lines.push(panel_line(
                    format!("  do {}", panel_rule_preview(&job.action, width)),
                    s.muted,
                    width,
                ));
                if job.skipped_overlap_count > 0 {
                    lines.push(panel_line(
                        format!("  skipped overlaps {}", job.skipped_overlap_count),
                        s.warn,
                        width,
                    ));
                }
            }
            if cron_jobs.len() > TRIGGER_PANEL_RULE_LIMIT {
                lines.push(panel_line(
                    format!("… {} more", cron_jobs.len() - TRIGGER_PANEL_RULE_LIMIT),
                    s.muted,
                    width,
                ));
            }
        }

        let hook_rows = self.panel_status.hook_points.len().max(1);
        let feature_rows = self.panel_status.trigger_features.len().max(1);
        // Skills + Triggers are variable above. Reserve enough rows for the lower static status
        // sections so MCP/Hooks/Runtime don't get clipped in ordinary tall terminals.
        let status_rows = 2 + 2 + 2 + hook_rows + 2 + feature_rows;
        while lines.len() + status_rows < height {
            lines.push(Line::raw(""));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("MCP".to_string(), s.section, width));
        if self.panel_status.mcp_servers == 0 {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            lines.push(panel_line(
                format!(
                    "servers {} · tools {}",
                    self.panel_status.mcp_servers, self.panel_status.mcp_tools
                ),
                s.badge,
                width,
            ));
            lines.push(panel_line(
                format!(
                    "notification hooks {}",
                    self.panel_status.mcp_notification_hooks
                ),
                s.muted,
                width,
            ));
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Hooks".to_string(), s.section, width));
        if self.panel_status.hook_points.is_empty() {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            for point in &self.panel_status.hook_points {
                lines.push(panel_line(format!("· {point}"), s.muted, width));
            }
        }

        lines.push(Line::raw(""));
        lines.push(panel_line("Runtime".to_string(), s.section, width));
        if self.panel_status.trigger_features.is_empty() {
            lines.push(panel_line("none".to_string(), s.muted, width));
        } else {
            for feature in &self.panel_status.trigger_features {
                lines.push(panel_line(format!("• {feature}"), s.muted, width));
            }
        }
        lines
    }
}
