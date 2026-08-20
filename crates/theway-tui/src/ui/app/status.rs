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
        Paragraph::new(Line::styled(text, Style::default().fg(Color::DarkGray)))
    }

    /// Busy band: nine independently styled middle dots show the rainbow
    /// snake in one terminal row, followed by the working label and stats.
    fn render_busy_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let tick = self.spinner_frame as u64;
        let cps = self.cps_meter.cps();
        let snake = snake_loader::snake_frame(self.spinner.step(), cps);
        let track_x = area.x.saturating_add(1);
        for (slot, position) in snake_loader::TRACK_ORDER.iter().enumerate() {
            let cell = &snake.cells[*position];
            let x = track_x.saturating_add(slot as u16);
            if x >= area.right() {
                break;
            }
            let mut style = Style::default().fg(cell.fg).bg(cell.bg);
            if cell.lit > 0.5 {
                style = style.add_modifier(Modifier::BOLD);
            }
            frame
                .buffer_mut()
                .set_string(x, area.y, cell.glyph.to_string(), style);
        }
        let label_x = track_x
            .saturating_add(snake_loader::TRACK_CELLS as u16)
            .saturating_add(2);
        if label_x < area.right() {
            let mut spans = vec![
                Span::styled("working", shimmer_style(tick)),
                Span::styled(
                    format!(" {}", self.elapsed_label()),
                    Style::default().fg(Color::DarkGray),
                ),
            ];
            if self.latest.queued_count > 0 {
                spans.push(Span::styled(
                    format!(" · {} queued", self.latest.queued_count),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if !self.follow {
                spans.push(Span::styled(
                    " · ↑scrolled",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            let line = Line::from(spans);
            let w = area.right().saturating_sub(label_x);
            frame.buffer_mut().set_line(label_x, area.y, &line, w);
        }
        self.render_busy_stats(frame, area);
    }

    /// Throughput stats on the right side of the busy rule:
    /// `84 char/s · input: 57.1k · output: 1.2k` (char/s from the meter;
    /// input/output from the recent context usage; no usage data → char/s
    /// only).
    fn render_busy_stats(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let usage = &self.latest.usage;
        let input = (usage.input_tokens > 0).then_some(usage.input_tokens);
        let output = (usage.output_tokens > 0).then_some(usage.output_tokens);
        let text = stats::busy_stats_text(self.cps_meter.cps(), input, output);
        let width = unicode_width::UnicodeWidthStr::width(text.as_str()) as u16;
        let right = area.right();
        if width == 0 || width >= right {
            return;
        }
        let x = right.saturating_sub(width).saturating_sub(1);
        frame
            .buffer_mut()
            .set_string(x, area.y, text, Style::default().fg(Color::DarkGray));
    }

    /// Elapsed time since the busy window began (`m s` after a minute).
    fn elapsed_label(&self) -> String {
        let Some(start) = self.busy_started else {
            return String::new();
        };
        let secs = start.elapsed().as_secs_f32();
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            format!("{}m {:.1}s", secs as u32 / 60, secs % 60.0)
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
        let area = frame.area();
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
