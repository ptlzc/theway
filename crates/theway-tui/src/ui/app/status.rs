impl App {
    fn status_line(&self, width: usize, _max_scroll: usize) -> Paragraph<'static> {
        let queue = if self.latest.queued_count == 0 {
            String::new()
        } else {
            format!(" · {} queued", self.latest.queued_count)
        };
        let status = if !self.connected {
            format!("daemon offline{queue}")
        } else {
            format!("ready{queue}")
        };
        let scrolled = if self.follow { "" } else { " ↑scrolled" };
        let label = format!(" {status}{scrolled} ");
        let mut text = label.clone();
        let used = unicode_width::UnicodeWidthStr::width(label.as_str());
        if width > used {
            text.push_str(&"─".repeat(width - used));
        }
        Paragraph::new(Line::styled(text, Style::default().fg(self.theme.statusbar.fg)))
    }

    /// Busy band: one rotating rainbow Braille glyph followed by one
    /// left-aligned working/status cluster. The glyph stays in one terminal
    /// cell across frames.
    fn render_busy_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let spinner = snake_loader::braille_frame(self.spinner.step());
        let track_x = area.x.saturating_add(1);
        if track_x < area.right() {
            frame
                .buffer_mut()
                .set_string(
                    track_x,
                    area.y,
                    spinner.glyph.to_string(),
                    Style::default().fg(spinner.fg),
                );
        }
        let label_x = track_x.saturating_add(3);
        if label_x < area.right() {
            let band = self.theme.statusbar;
            let mut spans = vec![Span::styled(
                "working",
                Style::default().fg(band.busy),
            )];
            let session_usage = &self.latest.session_usage;
            let stats = if session_usage.total_input_tokens > 0 {
                stats::busy_stats_text_with_session(
                    self.token_meter.cps(),
                    session_usage.total_input_tokens,
                    session_usage.output_tokens,
                    session_usage.provider_cache_hit_rate,
                    session_usage.prefix_cache_hit_rate,
                )
            } else {
                let usage = &self.latest.usage;
                let input = (usage.total_input_tokens > 0).then_some(usage.total_input_tokens);
                let output = (usage.output_tokens > 0).then_some(usage.output_tokens);
                stats::busy_stats_text(self.token_meter.cps(), input, output)
            };
            spans.push(Span::styled(
                format!(" · {stats}"),
                Style::default().fg(band.fg),
            ));
            // Issue #78: DAG / subagent / live-shell counters in the busy band.
            // `[n graph]` counts Running DAG runs and appears only while the
            // DAG band is Hidden (issue #76); `[n sub]` counts Running
            // subagents and `[n shell]` the live background shells, both shown
            // only when non-zero.
            let running_dags = self
                .latest
                .dags
                .iter()
                .filter(|d| d.status == "running")
                .count();
            if self.dag_band_mode == crate::ui::DagBandMode::Hidden && running_dags > 0 {
                spans.push(Span::styled(
                    format!(" · [{running_dags} graph]"),
                    Style::default().fg(band.fg),
                ));
            }
            let running_subs = self
                .latest
                .subagents
                .iter()
                .filter(|s| s.status == "running")
                .count();
            if running_subs > 0 {
                spans.push(Span::styled(
                    format!(" · [{running_subs} sub]"),
                    Style::default().fg(band.fg),
                ));
            }
            if self.latest.shell_count > 0 {
                spans.push(Span::styled(
                    format!(" · [{} shell]", self.latest.shell_count),
                    Style::default().fg(band.fg),
                ));
            }
            let line = Line::from(spans);
            let w = area.right().saturating_sub(label_x);
            frame.buffer_mut().set_line(label_x, area.y, &line, w);
        }
    }

    fn render_completions(&self, frame: &mut ratatui::Frame, status_area: Rect) {
        if self.completions.is_empty() {
            return;
        }
        // Issue #46: the highlight may sit anywhere in the full match list,
        // so the popup renders a fixed window starting at
        // `completion_scroll` and matches the highlight by absolute index.
        let scroll = self
            .completion_scroll
            .min(self.completions.len().saturating_sub(1));
        let shown = (self.completions.len() - scroll).min(COMPLETION_POPUP_MAX);
        let height = shown as u16 + 2; // borders
        let area = self.theme.screen.inset(frame.area());
        let y = status_area.y.saturating_sub(height).max(area.y);
        let width = area.width.clamp(10, 60);
        let rect = Rect {
            x: area.x,
            y,
            width,
            height,
        };
        let items: Vec<ListItem> = self
            .completions
            .iter()
            .skip(scroll)
            .take(shown)
            .enumerate()
            .map(|(i, c)| {
                let selected = scroll + i == self.completion_idx % self.completions.len();
                let style = if selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                ListItem::new(Line::styled(c.clone(), style))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("commands (Tab)")
                .border_style(Style::default().fg(Color::DarkGray)),
        );
        frame.render_widget(Clear, rect);
        frame.render_widget(list, rect);
    }
}
