impl<'a, 'b, 'syn, 'oc> MarkdownParser<'a, 'b, 'syn, 'oc> {
    /// Format a buffered table into lines with box-drawing borders.
    fn format_table(&self, state: &TableState) -> FormattedTable {
        let borders = TableBorders::BOX;
        let padding = 1;

        // Style already adapted - no need to call adapt_style again
        let border_style: ratatui::style::Style = self.ms.rule.style_into().dim();

        let all_rows: Vec<&Vec<StyledCell>> = std::iter::once(&state.header)
            .chain(state.rows.iter())
            .filter(|r| !r.is_empty())
            .collect();

        if all_rows.is_empty() {
            return FormattedTable::default();
        }

        let num_cols = all_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return FormattedTable::default();
        }

        let mut col_widths: Vec<usize> = vec![0; num_cols];
        for row in &all_rows {
            for (col_idx, cell) in row.iter().enumerate() {
                let text = cell.plain_text();
                let cell_width = text
                    .split('\n')
                    .map(unicode_display_width)
                    .max()
                    .unwrap_or(0);
                if col_idx < col_widths.len() {
                    col_widths[col_idx] = col_widths[col_idx].max(cell_width);
                }
            }
        }

        // Constrain column widths to fit within max_table_width if set.
        // Table width = 1 (left border) + sum(col_width + 2*padding) + (num_cols-1) separators + 1 (right border)
        //             = 1 + sum(col_width) + num_cols * 2 * padding + (num_cols - 1) + 1
        //             = num_cols * (2 * padding + 1) + sum(col_width) + 2 - 1
        if let Some(max_width) = self.max_table_width {
            let overhead = num_cols * (2 * padding + 1) + 1; // borders + padding
            let content_budget = max_width.saturating_sub(overhead);
            let total_content: usize = col_widths.iter().sum();

            if total_content > content_budget && total_content > 0 {
                // Compute per-column minimum widths: the longest unbreakable
                // word across all cells in each column.  The word separator
                // determines what counts as unbreakable (e.g. "LongalphaToken",
                // "$145,000", "ID-AA1001").
                let mut min_col_widths: Vec<usize> = vec![1; num_cols];
                // Per-column hard floors: the widest single grapheme (0 for
                // empty columns) — the narrowest width at which cell text can
                // still reflow without losing content.
                let mut hard_floors: Vec<usize> = vec![0; num_cols];
                for row in &all_rows {
                    for (col, cell) in row.iter().enumerate() {
                        if col >= num_cols {
                            break;
                        }
                        let text = cell.plain_text();
                        for word in cell_word_separator(&text) {
                            let w = unicode_display_width(word.word);
                            min_col_widths[col] = min_col_widths[col].max(w);
                        }
                        if !text.is_empty()
                            && let Some(floor) = hard_floors.get_mut(col)
                        {
                            let widest_grapheme = text
                                .graphemes(true)
                                .map(unicode_display_width)
                                .max()
                                .unwrap_or(0);
                            *floor = (*floor).max(widest_grapheme.max(1));
                        }
                    }
                }

                let min_total: usize = min_col_widths.iter().sum();
                let hard_total: usize = hard_floors.iter().sum();

                // Grow from the word minimums toward natural widths when the
                // word minimums fit the budget; otherwise restart from the
                // grapheme floors so long unbreakable tokens reflow inside
                // their cells instead of pushing the table past the budget.
                // When even the grapheme floors cannot fit, keep the word
                // minimums: downstream clipping remains the safety net.
                let (base_widths, target_widths) =
                    if min_total > content_budget && hard_total <= content_budget {
                        (hard_floors, min_col_widths)
                    } else {
                        (min_col_widths, col_widths.clone())
                    };
                let base_total: usize = base_widths.iter().sum();
                let extra_budget = content_budget.saturating_sub(base_total);

                // How much each column *wants* above its base.
                let extra_wants: Vec<usize> = target_widths
                    .iter()
                    .zip(&base_widths)
                    .map(|(&target, &base)| target.saturating_sub(base))
                    .collect();
                let total_extra_want: usize = extra_wants.iter().sum();

                let mut new_widths = base_widths.clone();
                if total_extra_want > 0 && extra_budget > 0 {
                    // Distribute proportionally.
                    for (width, &want) in new_widths.iter_mut().zip(&extra_wants) {
                        let share = (want as f64 * extra_budget as f64 / total_extra_want as f64)
                            .floor() as usize;
                        *width += share;
                    }

                    // Hand out any remaining columns (from floor rounding)
                    // to columns with the most unmet want, one at a time.
                    let used: usize = new_widths.iter().sum();
                    let mut remaining = content_budget.saturating_sub(used);
                    if remaining > 0 {
                        let mut indices: Vec<usize> = (0..num_cols).collect();
                        // Sort by unmet want descending.
                        let unmet = |i: usize| {
                            target_widths
                                .get(i)
                                .copied()
                                .unwrap_or(0)
                                .saturating_sub(new_widths.get(i).copied().unwrap_or(0))
                        };
                        indices.sort_by_key(|&index| std::cmp::Reverse(unmet(index)));
                        for &idx in &indices {
                            if remaining == 0 {
                                break;
                            }
                            // Don't grow beyond this pass's target width.
                            let target = target_widths.get(idx).copied().unwrap_or(0);
                            if let Some(width) = new_widths.get_mut(idx)
                                && *width < target
                            {
                                *width += 1;
                                remaining -= 1;
                            }
                        }
                    }
                }

                col_widths = new_widths;
            }
        }

        let alignments: Vec<_> = (0..num_cols)
            .map(|i| {
                state
                    .alignments
                    .get(i)
                    .copied()
                    .unwrap_or(pulldown_cmark::Alignment::None)
            })
            .collect();

        let mut lines = Vec::new();
        let mut styled_lines = Vec::new();
        let mut line_source_offsets: Vec<usize> = Vec::new();
        let mut hyperlinks: Vec<TableHyperlink> = Vec::new();

        // Source line layout within a table:
        //   offset 0: header row   (| Col A | Col B |)
        //   offset 1: separator    (|-------|-------|)
        //   offset 2+: body rows   (| val1  | val2  |)
        let header_offset = 0usize;
        let separator_offset = 1usize;

        // Top border — belongs to the header line
        let top_border = self.format_border_line(
            &col_widths,
            padding,
            borders.c_tl(),
            borders.t_t(),
            borders.c_tr(),
            borders.h(),
        );
        styled_lines.push(Line::styled(top_border.clone(), border_style));
        lines.push(top_border);
        line_source_offsets.push(header_offset);

        // Header row
        if !state.header.is_empty() {
            let (row_plains, row_styleds, row_links) = self.format_styled_content_lines(
                &state.header,
                &col_widths,
                &alignments,
                padding,
                borders.v(),
                border_style,
                true,
            );
            let base_line = styled_lines.len();
            for (p, s) in row_plains.into_iter().zip(row_styleds) {
                lines.push(p);
                styled_lines.push(s);
                line_source_offsets.push(header_offset);
            }
            for mut link in row_links {
                link.line_offset += base_line;
                hyperlinks.push(link);
            }

            // Header separator
            let sep = self.format_border_line(
                &col_widths,
                padding,
                borders.t_l(),
                borders.x(),
                borders.t_r(),
                borders.h(),
            );
            styled_lines.push(Line::styled(sep.clone(), border_style));
            lines.push(sep);
            line_source_offsets.push(separator_offset);
        }

        // Body rows
        for (i, row) in state.rows.iter().enumerate() {
            let row_offset = separator_offset + 1 + i; // offset 2, 3, ...

            let (row_plains, row_styleds, row_links) = self.format_styled_content_lines(
                row,
                &col_widths,
                &alignments,
                padding,
                borders.v(),
                border_style,
                false,
            );
            let base_line = styled_lines.len();
            for (p, s) in row_plains.into_iter().zip(row_styleds) {
                lines.push(p);
                styled_lines.push(s);
                line_source_offsets.push(row_offset);
            }
            for mut link in row_links {
                link.line_offset += base_line;
                hyperlinks.push(link);
            }

            // Row divider between body rows (not after last row)
            if i < state.rows.len().saturating_sub(1) {
                let row_sep = self.format_border_line(
                    &col_widths,
                    padding,
                    borders.t_l(),
                    borders.x(),
                    borders.t_r(),
                    borders.h(),
                );
                styled_lines.push(Line::styled(row_sep.clone(), border_style));
                lines.push(row_sep);
                line_source_offsets.push(row_offset);
            }
        }

        // Bottom border — belongs to the last body row
        let last_row_offset = separator_offset + state.rows.len();
        let bottom_border = self.format_border_line(
            &col_widths,
            padding,
            borders.c_bl(),
            borders.t_b(),
            borders.c_br(),
            borders.h(),
        );
        styled_lines.push(Line::styled(bottom_border.clone(), border_style));
        lines.push(bottom_border);
        line_source_offsets.push(last_row_offset);

        FormattedTable {
            lines,
            styled_lines,
            line_source_offsets,
            hyperlinks,
        }
    }

    /// Word-wrap a cell's plain text into lines of at most `width` display columns.
    /// Returns a Vec of Strings, one per visual line (never an empty Vec).
    ///
    /// Delegates to `textwrap::wrap` with a custom word separator that allows
    /// line breaks after spaces, punctuation, and symbol characters — but never
    /// mid-word.  A single word wider than `width` is then hard-split on
    /// grapheme boundaries so no visual line exceeds the column width.
    pub(crate) fn wrap_cell_text(text: &str, width: usize) -> Vec<String> {
        if width == 0 {
            return vec![String::new()];
        }
        let opts = textwrap::Options::new(width)
            .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit)
            .word_separator(textwrap::WordSeparator::Custom(cell_word_separator))
            .break_words(false);
        let wrapped = textwrap::wrap(text, opts);
        let mut lines: Vec<String> = Vec::with_capacity(wrapped.len());
        for cow in wrapped {
            let line = cow.into_owned();
            if unicode_display_width(&line) <= width {
                lines.push(line);
                continue;
            }
            // An unbreakable word survived textwrap wider than the column
            // (break_words is off, and textwrap's char-based emergency split
            // can tear grapheme clusters): hard-split on grapheme boundaries
            // using the same display-width model as the table formatter.
            let mut piece = String::new();
            let mut piece_width = 0usize;
            for grapheme in line.graphemes(true) {
                let grapheme_width = unicode_display_width(grapheme);
                if piece_width > 0 && piece_width.saturating_add(grapheme_width) > width {
                    lines.push(std::mem::take(&mut piece));
                    piece_width = 0;
                }
                piece.push_str(grapheme);
                piece_width = piece_width.saturating_add(grapheme_width);
            }
            if !piece.is_empty() {
                lines.push(piece);
            }
        }
        if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        }
    }

    /// Format a table row that may span multiple visual lines (when cells wrap).
    ///
    /// Returns `(plain_lines, styled_lines, hyperlinks)` — one plain + styled
    /// entry per visual line, plus any hyperlinks discovered in cell spans.
    /// Hyperlink `line_offset`s are relative to the first visual line of
    /// this row (caller adds the absolute base to embed in the table).
    #[allow(clippy::too_many_arguments)]
    fn format_styled_content_lines(
        &self,
        cells: &[StyledCell],
        col_widths: &[usize],
        alignments: &[pulldown_cmark::Alignment],
        padding: usize,
        v: char,
        border_style: ratatui::style::Style,
        is_header: bool,
    ) -> (Vec<String>, Vec<Line<'static>>, Vec<TableHyperlink>) {
        // 1. Wrap each cell's text into lines constrained to col_widths[i].
        let wrapped_cells: Vec<Vec<String>> = (0..col_widths.len())
            .map(|i| {
                let text = cells.get(i).map(|c| c.plain_text()).unwrap_or_default();
                Self::wrap_cell_text(&text, col_widths[i])
            })
            .collect();

        // 2. Determine the number of visual lines (max wrapped lines across cells).
        let num_visual_lines = wrapped_cells.iter().map(|c| c.len()).max().unwrap_or(1);

        // 3. Build each visual line.
        let mut all_plains = Vec::with_capacity(num_visual_lines);
        let mut all_styled = Vec::with_capacity(num_visual_lines);
        let mut all_links: Vec<TableHyperlink> = Vec::new();

        // Monotonic per-column source cursors: each fragment is searched for
        // strictly after the previous fragment's match end, so repeated
        // substrings (e.g. a linked "aa" followed by a plain "aa") can never
        // re-match earlier bytes once textwrap has eaten boundary whitespace.
        let mut source_cursors: Vec<usize> = vec![0; col_widths.len()];

        for vis_line in 0..num_visual_lines {
            let mut plain = String::new();
            let mut spans: Vec<Span<'static>> = Vec::new();
            // Running display column on this visual line; used to record
            // hyperlink column ranges in the table-local coordinate system.
            let mut display_col: usize = 0;

            plain.push(v);
            spans.push(Span::styled(v.to_string(), border_style));
            display_col += unicode_display_width(&v.to_string());

            for (i, width) in col_widths.iter().enumerate() {
                let cell_line_text = wrapped_cells[i]
                    .get(vis_line)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let cell_line_width = unicode_display_width(cell_line_text);
                let total_padding = width.saturating_sub(cell_line_width);

                let alignment = alignments
                    .get(i)
                    .copied()
                    .unwrap_or(pulldown_cmark::Alignment::None);
                let (left_pad, right_pad) = match alignment {
                    pulldown_cmark::Alignment::Left | pulldown_cmark::Alignment::None => {
                        (0, total_padding)
                    }
                    pulldown_cmark::Alignment::Right => (total_padding, 0),
                    pulldown_cmark::Alignment::Center => {
                        let left = total_padding / 2;
                        (left, total_padding - left)
                    }
                };

                // Left padding
                let left_space = " ".repeat(padding + left_pad);
                let left_space_width = unicode_display_width(&left_space);
                plain.push_str(&left_space);
                spans.push(Span::raw(left_space));
                display_col += left_space_width;

                // Cell text — slice original styled spans to match this
                // visual line's character range, preserving per-span formatting
                // (bold, italic, code, link) across wrap boundaries.
                if !cell_line_text.is_empty() {
                    if let Some(cell) = cells.get(i) {
                        // Find the byte offset of this visual line within the full
                        // cell plain text, then emit styled spans covering that range.
                        let full_text = cell.plain_text();
                        // Search from the previous fragment's match end so
                        // whitespace textwrap ate cannot make `.find`
                        // re-match an earlier overlapping occurrence of
                        // this fragment.
                        let cursor = floor_char_boundary(
                            &full_text,
                            source_cursors.get(i).copied().unwrap_or(0),
                        );
                        let line_start = full_text
                            .get(cursor..)
                            .and_then(|rest| rest.find(cell_line_text))
                            .map(|off| cursor + off)
                            .unwrap_or(cursor);
                        let line_end = (line_start + cell_line_text.len()).min(full_text.len());
                        if let Some(next_cursor) = source_cursors.get_mut(i) {
                            *next_cursor = line_end;
                        }

                        // Walk the cell's spans, emitting the slice that overlaps
                        // [line_start..line_end].
                        let mut offset = 0usize;
                        for cell_span in &cell.spans {
                            let span_start = offset;
                            let span_end = offset + cell_span.text.len();
                            offset = span_end;

                            // Intersect [span_start..span_end] with [line_start..line_end]
                            let start = span_start.max(line_start);
                            let end = span_end.min(line_end);
                            if start >= end {
                                continue;
                            }

                            let Some(slice) = full_text.get(start..end) else {
                                continue;
                            };
                            if slice.is_empty() {
                                continue;
                            }

                            let mut style: ratatui::style::Style = self.ms.text.style_into();
                            if is_header || cell_span.bold {
                                style = style.bold();
                            }
                            if cell_span.italic {
                                style = style.italic();
                            }
                            if cell_span.code {
                                style = self.ms.inline_code_inner.style_into();
                            }
                            if let Some((url, id)) = &cell_span.link {
                                // Apply link styling additively (preserves
                                // bold/italic if combined).  link_text style
                                // typically adds underline + accent color so
                                // the cell visually matches paragraph link
                                // rendering.
                                let link_style: ratatui::style::Style =
                                    self.ms.link_text.style_into();
                                style = style.patch(link_style);

                                let slice_width = unicode_display_width(slice);
                                all_links.push(TableHyperlink {
                                    line_offset: vis_line,
                                    column_range: display_col..(display_col + slice_width),
                                    url: url.clone(),
                                    id: *id,
                                });
                            }
                            let slice_width = unicode_display_width(slice);
                            plain.push_str(slice);
                            spans.push(Span::styled(slice.to_string(), style));
                            display_col += slice_width;
                        }
                    } else {
                        plain.push_str(cell_line_text);
                        spans.push(Span::raw(cell_line_text.to_string()));
                        display_col += cell_line_width;
                    }
                }

                // Right padding
                let right_space = " ".repeat(right_pad + padding);
                let right_space_width = unicode_display_width(&right_space);
                plain.push_str(&right_space);
                spans.push(Span::raw(right_space));
                display_col += right_space_width;

                // Column separator
                plain.push(v);
                spans.push(Span::styled(v.to_string(), border_style));
                display_col += unicode_display_width(&v.to_string());
            }

            all_plains.push(plain);
            all_styled.push(Line::from(spans));
        }

        (all_plains, all_styled, all_links)
    }

    fn format_border_line(
        &self,
        col_widths: &[usize],
        padding: usize,
        left: char,
        mid: char,
        right: char,
        h: char,
    ) -> String {
        let mut line = String::new();
        line.push(left);
        for (i, &width) in col_widths.iter().enumerate() {
            let total_width = width + padding * 2;
            for _ in 0..total_width {
                line.push(h);
            }
            if i < col_widths.len() - 1 {
                line.push(mid);
            }
        }
        line.push(right);
        line
    }
}
