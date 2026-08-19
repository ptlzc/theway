impl DemoApp {
    fn render(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        terminal.draw(|f| {
            let area = f.area();

            if self.line_select.is_some() {
                // ── Line select layout: preview + hints + prompt + status ──
                let [preview_area, hints_area, prompt_outer, status_area] = Layout::vertical([
                    Constraint::Min(5),
                    Constraint::Length(1),
                    Constraint::Length(5),
                    Constraint::Length(1),
                ])
                .areas(area);

                // Update viewport height in the mode so scrolling works correctly.
                if let Some(mode) = self.line_select.as_mut() {
                    // Reserve 2 rows for border.
                    mode.viewport_height = preview_area.height.saturating_sub(2) as usize;
                }

                self.render_line_select(f.buffer_mut(), preview_area);
                self.render_line_select_hints(f.buffer_mut(), hints_area);
                self.render_prompt(f, prompt_outer);

                let status = Paragraph::new(Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(&self.status, Style::default().fg(Color::DarkGray)),
                ]));
                status.render(status_area, f.buffer_mut());
            } else {
                // ── Normal layout ──
                let info_rows: u16 = 16;
                let fs_rows = if self.fs_active {
                    self.file_search.dropdown_height()
                } else {
                    0
                };
                // Ensure the prompt gets at least 5 rows (3 inner + border).
                let min_prompt: u16 = 5;
                let fixed = info_rows + 1 + fs_rows + 1;
                let remaining = area.height.saturating_sub(fixed);
                let half = (remaining / 2).max(min_prompt);

                let [
                    info_area,
                    _gap,
                    raw_buf_area,
                    fs_area,
                    prompt_outer,
                    status_area,
                ] = Layout::vertical([
                    Constraint::Length(info_rows),
                    Constraint::Length(1),
                    Constraint::Length(half),
                    Constraint::Length(fs_rows),
                    Constraint::Length(half),
                    Constraint::Length(1),
                ])
                .areas(area);

                self.render_info(f.buffer_mut(), info_area);
                self.render_raw_buffer(f.buffer_mut(), raw_buf_area);

                if fs_rows > 0 {
                    self.render_file_search(f.buffer_mut(), fs_area);
                }

                self.render_prompt(f, prompt_outer);

                let status = Paragraph::new(Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(&self.status, Style::default().fg(Color::DarkGray)),
                ]));
                status.render(status_area, f.buffer_mut());
            }

            // ── Cursor management (inside draw) ──
            //
            // By calling set_cursor_position inside the draw closure,
            // ratatui emits show_cursor + set_cursor_position WITHOUT
            // the hide_cursor that happens when no cursor is set.  This
            // avoids the hide→show cycle that resets the terminal's
            // blink timer every frame.
            let want_cursor = if self.line_select.is_none() {
                self.textarea
                    .cursor_pos_with_state(self.textarea_area, self.textarea_state)
            } else {
                None
            };

            if let Some((cx, cy)) = want_cursor {
                f.set_cursor_position(ratatui::layout::Position { x: cx, y: cy });
            }
        })?;

        Ok(())
    }

    /// Render the prompt box (shared between normal and line-select layouts).
    fn render_prompt(&mut self, f: &mut ratatui::Frame<'_>, prompt_outer: Rect) {
        let prompt_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(
                " Prompt ",
                Style::default().fg(Color::White).bold(),
            ));
        let prompt_inner = prompt_block.inner(prompt_outer);
        prompt_block.render(prompt_outer, f.buffer_mut());

        if prompt_inner.width > 2 {
            let [char_area, textarea_area] =
                Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(prompt_inner);

            let prompt_char = Span::styled("❯ ", Style::default().fg(Color::Magenta).bold());
            f.buffer_mut().set_string(
                char_area.x,
                char_area.y,
                &prompt_char.content,
                prompt_char.style,
            );

            StatefulWidgetRef::render_ref(
                &(&self.textarea),
                textarea_area,
                f.buffer_mut(),
                &mut self.textarea_state,
            );

            // Store the textarea render area for mouse mapping.
            self.textarea_area = textarea_area;
        }
    }

    /// Render the file preview with line numbers and selection highlighting.
    fn render_line_select(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let Some(mode) = &self.line_select else {
            return;
        };

        // Title shows file path + range if any.
        let title = match mode.effective_range() {
            Some(r) if r.start() == r.end() => format!(" {} :{} ", mode.file_path, r.start()),
            Some(r) => format!(" {} :{}-{} ", mode.file_path, r.start(), r.end()),
            None => format!(" {} ", mode.file_path),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(80, 80, 80)))
            .title(Span::styled(
                title,
                Style::default().fg(Color::White).bold(),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 6 || inner.height == 0 {
            return;
        }

        let total = mode.total_lines();
        let gutter_width = total.to_string().len();
        let code_start_col = inner.x + gutter_width as u16 + 1; // +1 for separator
        let code_width = inner.width.saturating_sub(gutter_width as u16 + 1);

        // Styles.
        let gutter_style = Style::default().fg(Color::Rgb(80, 80, 80));
        let gutter_cursor_style = Style::default().fg(Color::Yellow);
        let code_style = Style::default().fg(Color::Rgb(200, 200, 200));
        let cursor_bg = Style::default().fg(Color::White).bg(Color::Rgb(50, 50, 60));
        let selecting_bg = Style::default().fg(Color::White).bg(Color::Rgb(30, 60, 30));
        let locked_bg = Style::default().fg(Color::White).bg(Color::Rgb(70, 35, 35));
        let sep = "│";

        for row in 0..inner.height as usize {
            let line_idx = mode.scroll_top + row;
            if line_idx >= total {
                break;
            }
            let y = inner.y + row as u16;
            let line_num = line_idx + 1; // 1-indexed for display

            // Determine line style.
            let is_cursor = line_idx == mode.cursor_line;
            let is_sel = mode.is_selected(line_idx);

            let line_style = if is_cursor {
                cursor_bg
            } else if is_sel {
                match mode.selection {
                    SelectionState::Selecting(_) => selecting_bg,
                    SelectionState::Locked(_, _) => locked_bg,
                    SelectionState::None => code_style,
                }
            } else {
                code_style
            };

            // Line number gutter.
            let num_str = format!("{:>width$}", line_num, width = gutter_width);
            let g_style = if is_cursor {
                gutter_cursor_style
            } else {
                gutter_style
            };
            buf.set_string(inner.x, y, &num_str, g_style);
            buf.set_string(inner.x + gutter_width as u16, y, sep, gutter_style);

            // Code content.
            let content = &mode.lines[line_idx];
            // Fill the entire code area with background first if highlighted.
            if is_cursor || is_sel {
                for col in 0..code_width {
                    buf.set_string(code_start_col + col, y, " ", line_style);
                }
            }
            // Render the actual text (truncate to fit).
            let display: String = content.chars().take(code_width as usize).collect();
            buf.set_string(code_start_col, y, &display, line_style);
        }

        // Render the goto-line input at the bottom-right of the preview if active.
        if !mode.goto_buf.is_empty() {
            let goto_str = format!(":{}", mode.goto_buf);
            let w = goto_str.len() as u16;
            let x = area.x + area.width.saturating_sub(w + 2);
            let y = area.y + area.height.saturating_sub(1);
            buf.set_string(x, y, &goto_str, Style::default().fg(Color::Yellow).bold());
        }
    }

    fn render_line_select_hints(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let k = Style::default().fg(Color::Yellow);
        let d = Style::default().fg(Color::DarkGray);
        let s = Style::default().fg(Color::Rgb(50, 50, 50));

        let hints = Line::from(vec![
            Span::styled(" j/k", k),
            Span::styled(" ↕  ", d),
            Span::styled("│", s),
            Span::styled(" C-u/d", k),
            Span::styled(" ½pg  ", d),
            Span::styled("│", s),
            Span::styled(" f/b", k),
            Span::styled(" pg  ", d),
            Span::styled("│", s),
            Span::styled(" v/V", k),
            Span::styled(" select  ", d),
            Span::styled("│", s),
            Span::styled(" 0‑9", k),
            Span::styled(" goto  ", d),
            Span::styled("│", s),
            Span::styled(" g/G", k),
            Span::styled(" top/bot  ", d),
            Span::styled("│", s),
            Span::styled(" Enter", k),
            Span::styled(" confirm  ", d),
            Span::styled("│", s),
            Span::styled(" Esc/q", k),
            Span::styled(" cancel", d),
        ]);

        Paragraph::new(hints).render(area, buf);
    }

    fn render_file_search(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green))
            .title(Span::styled(
                " @ File Search ",
                Style::default().fg(Color::Green).bold(),
            ));
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let match_style = Style::default().fg(Color::Yellow).bold();
        let selected_style = Style::default().fg(Color::White);
        let normal_style = Style::default().fg(Color::Gray);
        let marker_style = Style::default().fg(Color::Green).bold();
        let dim_style = Style::default().fg(Color::DarkGray);

        for (i, result) in self
            .file_search
            .results
            .iter()
            .enumerate()
            .take(inner.height as usize)
        {
            let is_selected = i == self.file_search.selected;
            let y = inner.y + i as u16;

            // Selection marker
            let marker = if is_selected { "▸ " } else { "  " };
            buf.set_string(inner.x, y, marker, marker_style);

            // Path with fuzzy-match highlighting
            let base_style = if is_selected {
                selected_style
            } else {
                normal_style
            };
            let mut col = inner.x + 2;
            for (ci, ch) in result.path.chars().enumerate() {
                if col >= inner.x + inner.width {
                    break;
                }
                let style = if result.indices.contains(&ci) {
                    match_style
                } else {
                    base_style
                };
                let s = ch.to_string();
                buf.set_string(col, y, &s, style);
                col += unicode_width::UnicodeWidthStr::width(s.as_str()) as u16;
            }

            // Show score on the right for selected item
            if is_selected && result.score > 0 {
                let score_str = format!(" [{}]", result.score);
                let score_w = score_str.len() as u16;
                if inner.width > score_w + 4 {
                    let sx = inner.x + inner.width - score_w;
                    buf.set_string(sx, y, &score_str, dim_style);
                }
            }
        }
    }

    fn render_raw_buffer(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        // Build the block title: "Raw Buffer" + clipboard summary.
        let mut title_spans = vec![Span::styled(
            " Raw Buffer ",
            Style::default().fg(Color::Rgb(120, 120, 120)),
        )];
        if let Some(clip) = self.textarea.clipboard() {
            let preview: String = clip.chars().take(40).collect();
            let suffix = if clip.chars().count() > 40 { "…" } else { "" };
            title_spans.push(Span::styled(
                format!("│ clipboard: {preview}{suffix} "),
                Style::default().fg(Color::Rgb(80, 140, 80)),
            ));
        }

        let raw_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .title(Line::from(title_spans));
        let inner = raw_block.inner(area);
        raw_block.render(area, buf);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let text = self.textarea.text();
        let elements = self.textarea.elements();
        let plain_style = Style::default().fg(Color::Rgb(160, 160, 160));
        let plain_nl = Style::default().fg(Color::Rgb(80, 80, 80));
        let elem_style = Style::default().fg(Color::Rgb(200, 140, 60));
        let elem_nl = Style::default().fg(Color::Rgb(120, 80, 30));

        let mut display_spans: Vec<Span<'_>> = Vec::new();
        let mut pos = 0;

        for elem in elements {
            if pos < elem.range.start {
                push_with_visible_newlines(
                    &text[pos..elem.range.start],
                    plain_style,
                    plain_nl,
                    &mut display_spans,
                );
            }
            push_with_visible_newlines(
                &text[elem.range.clone()],
                elem_style,
                elem_nl,
                &mut display_spans,
            );
            pos = elem.range.end;
        }

        if pos < text.len() {
            push_with_visible_newlines(&text[pos..], plain_style, plain_nl, &mut display_spans);
        }

        let line = Line::from(display_spans);
        let opts = RtOptions::new(inner.width as usize).break_words(true);
        let wrapped = word_wrap_line(&line, opts);

        let para = Paragraph::new(Text::from(wrapped));
        para.render(inner, buf);
    }

    fn render_info(&self, buf: &mut ratatui::buffer::Buffer, area: Rect) {
        let mut lines = vec![
            Line::from(vec![
                Span::styled("TextArea Demo", Style::default().fg(Color::White).bold()),
                Span::styled(
                    " — @-File-Search + Atomic Elements",
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(""),
        ];

        let bindings = vec![
            ("@query", "trigger file search"),
            ("Tab / Enter", "confirm file selection"),
            ("↑/↓ / C-p/C-n", "navigate results"),
            ("Esc", "dismiss search / quit"),
            ("Paste (Cmd+V)", "create paste element"),
            ("i", "inline element at cursor"),
            ("←/→", "navigate (jumps over elements)"),
            ("Backspace/Del", "delete (atomic for elements)"),
            ("Alt+←/→", "word navigation"),
            ("Ctrl+A/E", "beginning/end of line"),
            ("Ctrl+K/U", "kill to end/beginning of line"),
            ("Ctrl+Z", "undo"),
            ("Ctrl+Shift+Z/Y", "redo"),
            ("Ctrl+C", "clear (quit if empty)"),
        ];

        for (key, desc) in bindings {
            lines.push(Line::from(vec![
                Span::styled(format!("{:>16}", key), Style::default().fg(Color::Yellow)),
                Span::styled("  ", Style::default()),
                Span::styled(desc, Style::default().fg(Color::Gray)),
            ]));
        }

        let text = Text::from(lines);
        let para = Paragraph::new(text);
        para.render(area, buf);
    }
}
