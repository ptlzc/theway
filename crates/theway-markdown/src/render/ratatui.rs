impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    /// Render to ratatui Lines.
    ///
    /// If `pretty` is true, syntax markers are hidden.
    /// Returns rendered lines, line source map, and optional checkpoint.
    pub fn render_ratatui(&mut self, pretty: bool) -> (MarkdownRenderOutput, Option<Checkpoint>) {
        // Build render events
        let render_events = self.build_render_events();

        self.buffers.current_spans.clear();
        self.buffers.active_highlights.clear();

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut line_source_map: Vec<usize> = Vec::new();
        let mut hyperlinks: Vec<HyperlinkTarget> = Vec::new();

        let mut last_pos = 0;
        let mut replace: Option<usize> = None;
        let mut table_replace: Option<usize> = None;
        let mut mermaid_replace: Option<usize> = None;
        let mut skip_leading_newline = false;
        let mut in_hidden_code_block = false;
        let mut next_link_idx: usize = 0;
        // Running display-column tracker for the in-progress line.
        let mut cur_col_in_line: usize = 0;

        let checkpoint_info = self.last_checkpoint;
        let mut checkpoint_output_lines: Option<usize> = None;

        // Style already adapted - no need to call adapt_style again
        let code_bg_style: ratatui::style::Style = self.ms.code_background.style_into();

        let in_untagged_code = |pos: usize, buffers: &MarkdownBuffers| -> bool {
            buffers
                .untagged_code_ranges
                .iter()
                .any(|range| pos >= range.start && pos < range.end)
        };

        let mut current_source_line = 0usize;
        let mut last_line_count_pos = 0usize;
        let mut pending_line_is_code = false;

        let count_newlines_in_range = |from: usize, to: usize, text: &str| -> usize {
            if to <= from {
                return 0;
            }
            let to = to.min(text.len());
            let from = from.min(to);
            // Use as_bytes() to avoid panicking on non-char-boundary offsets.
            // This is safe because '\n' (0x0A) is a single-byte ASCII value
            // that can never appear as a UTF-8 continuation byte (0x80..0xBF).
            text.as_bytes()[from..to]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
        };

        for ev in &render_events {
            if replace.is_none()
                && table_replace.is_none()
                && mermaid_replace.is_none()
                && ev.pos > last_pos
            {
                // Check if we need to split text processing at the checkpoint boundary.
                // If last_pos < cp_byte <= ev.pos, we process in two parts:
                // 1. Process [last_pos..cp_byte], capture lines.len(), process [cp_byte..ev.pos]
                let split_at_checkpoint = checkpoint_output_lines.is_none()
                    && checkpoint_info
                        .map(|(_, cp_byte)| last_pos < cp_byte && cp_byte <= ev.pos)
                        .unwrap_or(false);

                let cp_byte = checkpoint_info.map(|(_, cp)| cp).unwrap_or(0);

                // Snap cp_byte to the nearest char boundary.  Checkpoint byte
                // offsets come from pulldown-cmark event ranges which should
                // always be char-aligned, but in edge cases (e.g., thematic
                // breaks followed by headings with multi-byte chars) the
                // position can land mid-character.  Snapping forward is safe
                // because it only affects where we split the text for line
                // counting — a few extra or fewer newlines in the first vs
                // second range doesn't change the total count.
                let cp_byte = {
                    let mut b = cp_byte;
                    while b < self.text.len() && !self.text.is_char_boundary(b) {
                        b += 1;
                    }
                    b
                };

                // Determine ranges to process
                let ranges: &[(usize, usize)] = if split_at_checkpoint {
                    // Process in two parts, capturing checkpoint between them
                    &[(last_pos, cp_byte), (cp_byte, ev.pos)]
                } else {
                    // Process as single range
                    &[(last_pos, ev.pos)]
                };

                for (range_idx, &(range_start, range_end)) in ranges.iter().enumerate() {
                    // After processing the first range when splitting, capture checkpoint.
                    // Flush any pending spans to `lines` first — content like a thematic
                    // break (`───`) may sit in `current_spans` without a trailing newline
                    // to flush it.  Without this flush, the checkpoint's `output_lines`
                    // count would be too low, causing the line to vanish on re-render.
                    if split_at_checkpoint && range_idx == 1 {
                        if !self.buffers.current_spans.is_empty() {
                            line_source_map.push(current_source_line);
                            let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
                            lines.push(line);
                            cur_col_in_line = 0;
                        }
                        checkpoint_output_lines = Some(lines.len());
                    }

                    if range_end <= range_start {
                        continue;
                    }

                    // Update source line counter
                    if range_start > last_line_count_pos {
                        current_source_line +=
                            count_newlines_in_range(last_line_count_pos, range_start, self.text);
                        last_line_count_pos = range_start;
                    }

                    let is_hidden = pretty
                        && all_hidden(
                            self.buffers
                                .active_highlights
                                .iter()
                                .map(|&i| self.buffers.highlights[i].style),
                        );

                    if is_hidden {
                        let at_line_start = range_start == 0
                            || self.text.as_bytes().get(range_start - 1) == Some(&b'\n');
                        if at_line_start {
                            // Check if this hidden block is a code fence (``` or ~~~).
                            // Only code fences need separator handling — heading markers
                            // (#) are also hidden at line start but are unpaired.
                            let hidden_text = self.text[range_start..range_end].trim_start();
                            let is_code_fence =
                                hidden_text.starts_with("```") || hidden_text.starts_with("~~~");

                            if is_code_fence {
                                // Emit a blank separator before an OPENING fence (not
                                // closing). Prevents adjacent blocks (e.g., list → code)
                                // from collapsing their visual boundary when the hidden
                                // fence markers are removed in pretty mode.
                                if !in_hidden_code_block
                                    && lines.last().is_some_and(|l| l.width() > 0)
                                {
                                    line_source_map.push(current_source_line);
                                    lines.push(Line::default());
                                    cur_col_in_line = 0;
                                }
                                in_hidden_code_block = !in_hidden_code_block;
                            }
                            skip_leading_newline = true;
                        }
                    } else {
                        let mut text = &self.text[range_start..range_end];
                        let mut text_start = range_start;

                        if skip_leading_newline && text.starts_with('\n') {
                            text = &text[1..];
                            text_start += 1;
                        }
                        skip_leading_newline = false;

                        if !text.is_empty() {
                            let style = merge_styles(
                                self.buffers
                                    .active_highlights
                                    .iter()
                                    .map(|&i| self.buffers.highlights[i].style),
                            );

                            let transformed = self.apply_transforms(text, range_start, pretty);
                            let ratatui_style: ratatui::style::Style = style.style_into();

                            let chunk_src_start = text_start;
                            let chunk_src_end = text_start + text.len();

                            // Advance the cursor past links that ended before
                            // this chunk starts, then check if any remaining
                            // link overlaps the chunk.  Skip all hyperlink
                            // bookkeeping when none does — keeps the no-link
                            // hot path identical to the pre-feature renderer.
                            while next_link_idx < self.buffers.link_targets.len()
                                && self.buffers.link_targets[next_link_idx].source_range.end
                                    <= chunk_src_start
                            {
                                next_link_idx += 1;
                            }
                            let chunk_has_links = next_link_idx < self.buffers.link_targets.len()
                                && self.buffers.link_targets[next_link_idx].source_range.start
                                    < chunk_src_end;

                            let chunk_links: Vec<ChunkLinkRange> = if chunk_has_links {
                                chunk_link_offsets(
                                    &self.buffers.link_targets,
                                    next_link_idx,
                                    chunk_src_start,
                                    chunk_src_end,
                                    pretty,
                                    &self.buffers.transforms,
                                )
                            } else {
                                Vec::new()
                            };

                            let mut byte_offset = text_start;
                            let mut seg_x_offset: usize = 0;
                            let is_in_code = in_untagged_code(text_start, self.buffers);
                            pending_line_is_code = is_in_code;
                            for (idx, segment) in transformed.split('\n').enumerate() {
                                if idx > 0 {
                                    line_source_map.push(current_source_line);
                                    let line =
                                        Line::from(std::mem::take(&mut self.buffers.current_spans));
                                    lines.push(if is_in_code {
                                        line.style(code_bg_style)
                                    } else {
                                        line
                                    });
                                    if byte_offset > last_line_count_pos {
                                        current_source_line += count_newlines_in_range(
                                            last_line_count_pos,
                                            byte_offset,
                                            self.text,
                                        );
                                        last_line_count_pos = byte_offset;
                                    }
                                    cur_col_in_line = 0;
                                }

                                if !chunk_links.is_empty() {
                                    emit_segment_hyperlinks(
                                        &chunk_links,
                                        &self.buffers.link_targets,
                                        segment,
                                        seg_x_offset,
                                        cur_col_in_line,
                                        lines.len(),
                                        &mut hyperlinks,
                                    );
                                }

                                if !segment.is_empty() {
                                    self.buffers
                                        .current_spans
                                        .push(Span::styled(segment.to_string(), ratatui_style));
                                    cur_col_in_line += unicode_display_width(segment);
                                }
                                byte_offset += segment.len() + 1;
                                seg_x_offset += segment.len() + 1;
                            }
                        }
                    }
                }
                last_pos = ev.pos;
            }

            match ev.kind {
                RenderEventKind::Replace => {
                    if ev.is_end && replace == Some(ev.index) {
                        replace = None;
                    } else if !ev.is_end && replace.is_none() && table_replace.is_none() {
                        replace = Some(ev.index);
                        let repl = &self.buffers.replaces[ev.index];

                        // Update source line to code start
                        if repl.range.start > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                repl.range.start,
                                self.text,
                            );
                        }
                        let code_start_source_line = current_source_line;

                        for (line_idx, line_spans) in repl.highlighted.iter().enumerate() {
                            current_source_line = code_start_source_line + line_idx;

                            for (syn_style, text) in line_spans {
                                let full_style = anstyle_syntect::to_anstyle(*syn_style);
                                let with_bg =
                                    full_style.bg_color(self.ms.code_background.get_bg_color());
                                // This is the only legitimate inline adapt_style call
                                // for dynamically created syntect+background combo
                                let adapted = adapt_style_for(with_bg, self.color_level);
                                let ratatui_style: ratatui::style::Style = adapted.style_into();

                                for (idx, segment) in text.split('\n').enumerate() {
                                    if idx > 0 {
                                        line_source_map.push(current_source_line);
                                        let line = Line::from(std::mem::take(
                                            &mut self.buffers.current_spans,
                                        ))
                                        .style(code_bg_style);
                                        lines.push(line);
                                        current_source_line += 1;
                                        cur_col_in_line = 0;
                                    }
                                    if !segment.is_empty() {
                                        self.buffers
                                            .current_spans
                                            .push(Span::styled(segment.to_string(), ratatui_style));
                                        cur_col_in_line += unicode_display_width(segment);
                                    }
                                }
                            }

                            if !self.buffers.current_spans.is_empty() {
                                line_source_map.push(current_source_line);
                                let line =
                                    Line::from(std::mem::take(&mut self.buffers.current_spans))
                                        .style(code_bg_style);
                                lines.push(line);
                                cur_col_in_line = 0;
                            }
                        }

                        last_pos = repl.range.end;
                        let newlines_in_code =
                            count_newlines_in_range(repl.range.start, repl.range.end, self.text);
                        current_source_line = code_start_source_line + newlines_in_code;
                        last_line_count_pos = repl.range.end;

                        if checkpoint_output_lines.is_none()
                            && let Some((_, cp_byte)) = checkpoint_info
                            && last_pos >= cp_byte
                        {
                            checkpoint_output_lines = Some(lines.len());
                        }
                    }
                }
                RenderEventKind::Table => {
                    if ev.is_end && table_replace == Some(ev.index) {
                        table_replace = None;
                    } else if !ev.is_end && table_replace.is_none() && pretty {
                        table_replace = Some(ev.index);
                        let trepl = &self.buffers.table_replaces[ev.index];

                        // Flush any in-progress inline spans first. Tables
                        // always start at a line boundary (no-op), but a
                        // display-math block replacement can occur
                        // mid-paragraph (`text $$x$$ more`): without the
                        // flush, the pending "text " spans would be emitted
                        // AFTER the block lines.
                        if !self.buffers.current_spans.is_empty() {
                            line_source_map.push(current_source_line);
                            lines.push(Line::from(std::mem::take(&mut self.buffers.current_spans)));
                            // cur_col_in_line is reset unconditionally after
                            // the block lines are emitted below.
                        }

                        // Update source line to table start
                        if trepl.range.start > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                trepl.range.start,
                                self.text,
                            );
                        }
                        let table_start_source_line = current_source_line;
                        let table_base_line = lines.len();

                        for (line_idx, styled_line) in trepl.styled_lines.iter().enumerate() {
                            let offset = trepl
                                .line_source_offsets
                                .get(line_idx)
                                .copied()
                                .unwrap_or(0);
                            current_source_line = table_start_source_line + offset;
                            line_source_map.push(current_source_line);
                            lines.push(styled_line.clone());
                        }
                        // Translate table-local hyperlink coordinates into
                        // absolute line indices and append to the global list.
                        for link in &trepl.hyperlinks {
                            hyperlinks.push(HyperlinkTarget {
                                line_index: table_base_line + link.line_offset,
                                column_range: link.column_range.clone(),
                                url: link.url.clone(),
                                id: link.id,
                            });
                        }
                        // Table emits whole pre-rendered lines; reset col so
                        // any subsequent inline content starts at column 0.
                        cur_col_in_line = 0;

                        last_pos = trepl.range.end;
                        let newlines_in_table =
                            count_newlines_in_range(trepl.range.start, trepl.range.end, self.text);
                        current_source_line = table_start_source_line + newlines_in_table;
                        last_line_count_pos = trepl.range.end;

                        if checkpoint_output_lines.is_none()
                            && let Some((_, cp_byte)) = checkpoint_info
                            && last_pos >= cp_byte
                        {
                            checkpoint_output_lines = Some(lines.len());
                        }
                    }
                }
                RenderEventKind::Mermaid => {
                    if ev.is_end && mermaid_replace == Some(ev.index) {
                        mermaid_replace = None;
                    } else if !ev.is_end && mermaid_replace.is_none() && pretty {
                        mermaid_replace = Some(ev.index);
                        let mrepl = &self.buffers.mermaid_replaces[ev.index];

                        if mrepl.range.start > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                mrepl.range.start,
                                self.text,
                            );
                        }
                        let start_source_line = current_source_line;

                        for styled_line in &mrepl.styled_lines {
                            line_source_map.push(start_source_line);
                            lines.push(styled_line.clone());
                        }
                        cur_col_in_line = 0;

                        last_pos = mrepl.range.end;
                        let newlines =
                            count_newlines_in_range(mrepl.range.start, mrepl.range.end, self.text);
                        current_source_line = start_source_line + newlines;
                        last_line_count_pos = mrepl.range.end;

                        if checkpoint_output_lines.is_none()
                            && let Some((_, cp_byte)) = checkpoint_info
                            && last_pos >= cp_byte
                        {
                            checkpoint_output_lines = Some(lines.len());
                        }
                    }
                }
                RenderEventKind::Highlight => {
                    if ev.is_end {
                        self.buffers.active_highlights.retain(|&x| x != ev.index);
                    } else {
                        self.buffers.active_highlights.push(ev.index);
                    }
                }
            }
        }

        // Handle remaining text
        let len = self.text.len();
        if last_pos < len {
            // Apply force transforms only; non-force transforms have
            // never been applied in this trailing path and force
            // transforms preserve byte length so source offsets below
            // stay valid.
            let raw = &self.text[last_pos..len];
            let transformed = self.apply_transforms(raw, last_pos, false);
            debug_assert_eq!(transformed.len(), raw.len());
            let text: &str = &transformed;
            let is_only_whitespace = text.as_bytes().iter().all(u8::is_ascii_whitespace);

            if !(pretty && is_only_whitespace) {
                if last_pos > last_line_count_pos {
                    current_source_line +=
                        count_newlines_in_range(last_line_count_pos, last_pos, self.text);
                    last_line_count_pos = last_pos;
                }
                let chunk_src_start = last_pos;
                let chunk_src_end = last_pos + text.len();

                // Same cursor-skip pattern as the main path: keep the no-link
                // hot path identical to the pre-feature renderer.
                while next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.end <= chunk_src_start
                {
                    next_link_idx += 1;
                }
                let chunk_has_links = next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.start < chunk_src_end;

                // Trailing text bypasses apply_transforms (it's emitted raw),
                // so transformed offsets equal source offsets within the chunk.
                let chunk_links: Vec<ChunkLinkRange> = if chunk_has_links {
                    chunk_link_offsets(
                        &self.buffers.link_targets,
                        next_link_idx,
                        chunk_src_start,
                        chunk_src_end,
                        false,
                        &[],
                    )
                } else {
                    Vec::new()
                };

                let mut byte_offset = last_pos;
                let mut seg_x_offset: usize = 0;
                let is_in_code = in_untagged_code(last_pos, self.buffers);
                pending_line_is_code = is_in_code;

                for (idx, segment) in text.split('\n').enumerate() {
                    if idx > 0 {
                        line_source_map.push(current_source_line);
                        let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
                        lines.push(if is_in_code {
                            line.style(code_bg_style)
                        } else {
                            line
                        });
                        if byte_offset > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                byte_offset,
                                self.text,
                            );
                            last_line_count_pos = byte_offset;
                        }
                        cur_col_in_line = 0;
                    }

                    if !chunk_links.is_empty() {
                        emit_segment_hyperlinks(
                            &chunk_links,
                            &self.buffers.link_targets,
                            segment,
                            seg_x_offset,
                            cur_col_in_line,
                            lines.len(),
                            &mut hyperlinks,
                        );
                    }

                    if !segment.is_empty() {
                        self.buffers
                            .current_spans
                            .push(Span::raw(segment.to_string()));
                        cur_col_in_line += unicode_display_width(segment);
                    }
                    byte_offset += segment.len() + 1;
                    seg_x_offset += segment.len() + 1;
                }
            }
        }

        // Emit final line. Use the membership of the chunk that produced these spans:
        // an unterminated bare fence ends its range exactly at last_pos (EOF) and the
        // range check is end-exclusive, so recomputing here would drop the code bg.
        if !self.buffers.current_spans.is_empty() {
            line_source_map.push(current_source_line);
            let final_is_code = pending_line_is_code;
            let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
            lines.push(if final_is_code {
                line.style(code_bg_style)
            } else {
                line
            });
        }

        // If checkpoint wasn't captured during event processing, compute it based on
        // the number of newlines in the text up to checkpoint byte.
        // This handles cases where there are no events past the checkpoint (e.g., incomplete list items).
        if checkpoint_output_lines.is_none()
            && let Some((_, cp_byte)) = checkpoint_info
        {
            // Count newlines in text before the checkpoint byte.
            // Each newline ENDS a line, so N newlines = N complete lines.
            // However, we need to account for blank lines that are absorbed
            // into the block separator. The checkpoint is at the start of
            // the NEXT block, so lines from the frozen content should not
            // include any content that starts at or after cp_byte.
            //
            // More precise approach: count how many output lines have their
            // content entirely before cp_byte. This is tricky without tracking
            // each line's byte range.
            //
            // Logic:
            // - Each newline ENDS a line
            // - Use line_source_map to find output lines before checkpoint
            // - line_source_map[i] is the source line at which output line i was created
            // - source_line_at_cp is the source line containing cp_byte
            // - Output lines with source_line < source_line_at_cp are complete before checkpoint

            let source_line_at_cp = self.text[..cp_byte.min(self.text.len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();

            // When the checkpoint is at or past the end of the text, ALL output
            // lines belong to the frozen content (the entire input was consumed
            // by the checkpointed block).  Otherwise, output lines created at
            // source lines strictly before the checkpoint source line are frozen.
            let complete_lines = if cp_byte >= self.text.len() {
                lines.len()
            } else {
                line_source_map
                    .iter()
                    .take_while(|&&src_line| src_line < source_line_at_cp)
                    .count()
            };

            checkpoint_output_lines = Some(complete_lines.min(lines.len()));
        }

        let checkpoint = match (checkpoint_info, checkpoint_output_lines) {
            (Some((kind, source_bytes)), Some(output_lines)) => Some(Checkpoint {
                source_bytes,
                output_lines,
                kind,
            }),
            _ => None,
        };

        // Now that `line_source_map` is final, map each parsed code block's
        // body onto its rendered (pre-wrap) line range.
        let text = self.text;
        let code_blocks = crate::output::build_code_block_spans(
            text,
            &line_source_map,
            std::mem::take(&mut self.buffers.code_blocks),
        );

        (
            MarkdownRenderOutput {
                lines,
                line_source_map,
                hyperlinks,
                code_blocks,
            },
            checkpoint,
        )
    }
}
