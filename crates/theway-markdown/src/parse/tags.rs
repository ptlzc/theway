impl<'a, 'b, 'syn, 'oc> MarkdownParser<'a, 'b, 'syn, 'oc> {
    fn on_start(&mut self, tag: Tag<'a>, range: Range<usize>) {
        // Track nesting depth for checkpoint detection
        match &tag {
            Tag::BlockQuote(_) | Tag::List(_) | Tag::Item | Tag::Table(_) => {
                self.depth += 1;
            }
            _ => {}
        }
        if matches!(&tag, Tag::BlockQuote(_)) {
            self.bq_depth += 1;
        }

        let mut more = Vec::new();
        let style = match &tag {
            Tag::Paragraph => None,
            Tag::Heading { level, .. } => {
                let level_usize = (*level as usize).saturating_sub(1).min(5);
                let heading_text = &self.text[range.clone()];
                if let Some(marker_end) = heading_text.find(|c: char| c != '#' && c != ' ') {
                    let marker_range = range.start..range.start + marker_end;
                    more.push(Highlight {
                        style: Some(self.ms.heading_outer[level_usize]),
                        range: marker_range,
                    });
                    None
                } else {
                    Some(self.ms.heading_outer[level_usize])
                }
            }
            Tag::BlockQuote(_) => {
                // Transform the `>` belonging to THIS blockquote level to `│`.
                //
                // For nested blockquotes (`> > inner`), pulldown-cmark emits nested
                // BlockQuote events.  The outer event's range covers all lines,
                // so each line in the outer range has `>` at position 0.  The inner
                // event's range starts mid-line on the first line (after `> `) but
                // at column 0 on subsequent lines.  On those subsequent lines, the
                // outer `>` is included in the inner range, so we must skip it.
                //
                // Strategy: on each line, determine how many `>` characters belong
                // to outer blockquote levels (by checking if the line starts at a
                // real line boundary in the source).  If it does, skip (bq_depth-1)
                // `>`s.  If it starts mid-line (first fragment), skip none.
                let bq_text = &self.text[range.clone()];
                let mut pos = range.start;

                for line in bq_text.split_inclusive('\n') {
                    // Does this fragment start at a source line boundary?
                    let at_line_start =
                        pos == 0 || self.text.as_bytes().get(pos - 1) == Some(&b'\n');
                    // If at a line start, outer levels already have `>`s that
                    // we must skip.  If mid-line (first fragment of range),
                    // the outer `>`s are before the range so skip 0.
                    let skip = if at_line_start { self.bq_depth - 1 } else { 0 };

                    let mut found = 0usize;
                    for (byte_offset, ch) in line.char_indices() {
                        if ch == '>' {
                            if found == skip {
                                let gt_pos = pos + byte_offset;
                                self.buffers.transforms.push(Transform {
                                    range: gt_pos..gt_pos + 1,
                                    to: "│".to_string(),
                                    force: false,
                                });
                                more.push(Highlight {
                                    style: Some(self.ms.blockquote_outer),
                                    range: gt_pos..gt_pos + 1,
                                });
                                break;
                            }
                            found += 1;
                        }
                    }

                    pos += line.len();
                }

                // Return None - we've handled the styling via per-line highlights
                None
            }
            Tag::CodeBlock(code) => {
                // Track the fenced block so its body span can be reported once
                // the fence closes. The body starts just past the opening fence
                // line; an empty-body fence keeps this empty range. Indented
                // code blocks are not fences and report no span.
                self.pending_code_block = match code {
                    CodeBlockKind::Fenced(lang) => {
                        let body_start = self.text[range.start..]
                            .find('\n')
                            .map_or(range.end, |nl| range.start + nl + 1);
                        Some(PendingCodeBlock {
                            info: lang.to_string(),
                            body_range: body_start..body_start,
                            body_text: String::new(),
                            body_seen: false,
                        })
                    }
                    CodeBlockKind::Indented => None,
                };

                // pulldown-cmark reports the code-block range starting at the
                // fence marker (```), excluding any leading indentation on the
                // opening-fence line. That indentation is present whenever the
                // block is indented at the top level or nested inside a list.
                // Extend the hidden `code_outer` highlight back over it so the
                // whole fence line is hidden in pretty mode. Without this, the
                // indentation leaks onto the first rendered code line, and the
                // renderer's fence-start detection (which checks that the byte
                // before the fence is a newline) misfires — mistaking the
                // closing fence for an opening one and emitting a spurious
                // blank line. Only extend when the prefix is pure whitespace so
                // structural prefixes (e.g. a blockquote `> `) are left intact.
                let line_start = self.text[..range.start].rfind('\n').map_or(0, |p| p + 1);
                let fence_start = if self.text[line_start..range.start]
                    .bytes()
                    .all(|b| b == b' ' || b == b'\t')
                {
                    line_start
                } else {
                    range.start
                };
                more.push(Highlight {
                    style: Some(self.ms.code_outer),
                    range: fence_start..range.end,
                });
                match code {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        if let Some(r) =
                            find_substring(&self.text[range.clone()], lang, true, false)
                        {
                            let range = (r.start + range.start)..(r.end + range.start);
                            more.push(Highlight {
                                style: Some(self.ms.code_language),
                                range,
                            });
                        }
                    }
                    _ => (),
                }
                None
            }
            Tag::HtmlBlock => {
                // Don't syntax-highlight HTML blocks as code. In LLM output,
                // these are typically XML-like structural tags (e.g. <example>)
                // from system prompts, not actual HTML. Treating them as code
                // blocks (with background styling) causes visual inconsistency
                // because pulldown-cmark ends HTML blocks at blank lines,
                // making the first part look like code and the rest like text.
                None
            }
            Tag::List(_) => None,
            Tag::Item => {
                let item_text = &self.text[range.clone()];
                let trimmed = item_text.trim_start();
                let leading_ws = item_text.len() - trimmed.len();

                let marker_len = if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                    2
                } else if let Some(pos) = trimmed.find(". ") {
                    if pos > 0 && trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                        pos + 2
                    } else {
                        0
                    }
                } else if let Some(pos) = trimmed.find(") ") {
                    if pos > 0 && trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                        pos + 2
                    } else {
                        0
                    }
                } else {
                    0
                };

                if marker_len > 0 {
                    let marker_start = range.start + leading_ws;
                    let marker_end = marker_start + marker_len;
                    more.push(Highlight {
                        style: Some(self.ms.list_item),
                        range: marker_start..marker_end,
                    });

                    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                        self.buffers.transforms.push(Transform {
                            range: marker_start..marker_start + 1,
                            to: "•".to_string(),
                            force: false,
                        });
                    }
                }
                None
            }
            Tag::Table(alignments) => {
                self.table_state = Some(TableState::new(alignments.to_vec(), range.start));
                Some(self.ms.table_outer)
            }
            Tag::TableHead => {
                if let Some(ref mut state) = self.table_state {
                    state.in_header = true;
                }
                Some(self.ms.table_outer)
            }
            Tag::TableRow => {
                if let Some(ref mut state) = self.table_state {
                    state.current_row.clear();
                }
                Some(self.ms.table_outer)
            }
            Tag::TableCell => {
                if let Some(ref mut state) = self.table_state {
                    state.current_cell.clear();
                }
                self.push_highlight(None, &range);
                None
            }
            Tag::Emphasis => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_italic = true;
                }
                Some(self.ms.emphasis_outer)
            }
            Tag::Strong => {
                if let Some(ref mut state) = self.table_state {
                    state.cell_bold = true;
                }
                Some(self.ms.strong_outer)
            }
            Tag::Strikethrough => Some(self.ms.strikethrough_outer),
            Tag::Link {
                dest_url, title, ..
            }
            | Tag::Image {
                dest_url, title, ..
            } => {
                // Links inside a table cell go through the table renderer's
                // own hyperlink path (TableHyperlink in TableReplace).  The
                // paragraph link path (LinkTarget + chunk_link_offsets) can't
                // project links onto rendered table cells because the table
                // replace consumes the entire source range — no text chunk
                // ever covers the cell's link text.
                //
                // We still want a stable link `id` so terminal UIs can group
                // wrapped link fragments; assign one from the same counter
                // used by paragraph links and stash it in `cell_link` so the
                // following `Event::Text`s tag their CellSpans with it.
                if let Some(ref mut state) = self.table_state {
                    let id = self.link_id_counter;
                    self.link_id_counter += 1;
                    state.cell_link = Some((dest_url.to_string(), id));
                    self.tag_stack.push(tag);
                    return;
                }

                let tag_str = &self.text[range.clone()];

                if !title.is_empty() {
                    for t in [format!("\"{title}\""), format!("'{title}'")].map(CowStr::from) {
                        if let Some(r) = find_substring(tag_str, &t, true, true) {
                            let title_range = (r.start + range.start)..(r.end + range.start);
                            more.push(Highlight {
                                style: Some(self.ms.link_title),
                                range: title_range,
                            });
                        }
                    }
                }

                // We intentionally use allow_outside=true here (instead of the previous
                // pointer-based allow_outside=false) and then do an rfind on the prefix
                // before the (last) dest_url occurrence. This is required because dest_url
                // may be a CowStr::Owned (after percent-decoding or HTML entity expansion)
                // and therefore may not be a sub-slice of tag_str. The rfind on the strict
                // prefix guarantees we find the *structural* `](` closer even when the link
                // text, title, or the dest literal itself contains the byte sequence `](`.
                let url_rel_opt = find_substring(tag_str, dest_url, true, true);
                if let Some(r) = &url_rel_opt {
                    let url_range = (r.start + range.start)..(r.end + range.start);
                    more.push(Highlight {
                        style: Some(self.ms.link_url),
                        range: url_range,
                    });
                }

                let bracket_pos_opt = url_rel_opt
                    .as_ref()
                    .and_then(|r| tag_str[..r.start].rfind("](").map(|p| p..p + 2));
                if let Some(bracket_pos) = bracket_pos_opt {
                    let open_bracket = if tag_str.starts_with("![") { 1 } else { 0 };
                    if open_bracket > 0 {
                        more.push(Highlight {
                            style: Some(self.ms.link_outer),
                            range: range.start..range.start + 1,
                        });
                    }
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: range.start + open_bracket..range.start + open_bracket + 1,
                    });
                    let text_start = range.start + open_bracket + 1;
                    let text_end = bracket_pos.start + range.start;
                    if text_end > text_start {
                        more.push(Highlight {
                            style: Some(self.ms.link_text),
                            range: text_start..text_end,
                        });
                    }
                    let bracket_abs = bracket_pos.start + range.start;
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: bracket_abs..bracket_abs + 2,
                    });
                    more.push(Highlight {
                        style: Some(self.ms.link_outer),
                        range: range.end - 1..range.end,
                    });
                    self.buffers.transforms.push(Transform {
                        range: range.start + open_bracket..range.start + open_bracket + 1,
                        to: "".to_string(),
                        force: false,
                    });
                    self.buffers.transforms.push(Transform {
                        range: bracket_abs..bracket_abs + 2,
                        to: " (".to_string(),
                        force: false,
                    });
                    if text_end > text_start {
                        self.buffers.link_targets.push(LinkTarget {
                            source_range: text_start..text_end,
                            url: dest_url.to_string(),
                            id: self.link_id_counter,
                        });
                        self.link_id_counter += 1;
                    }
                    None
                } else {
                    self.buffers.link_targets.push(LinkTarget {
                        source_range: range.clone(),
                        url: dest_url.to_string(),
                        id: self.link_id_counter,
                    });
                    self.link_id_counter += 1;
                    Some(self.ms.link_outer)
                }
            }
            _ => None,
        };

        if let Some(style) = style {
            self.push_highlight(Some(style), &range);
        }
        for hl in more {
            self.buffers.highlights.push(hl);
        }

        self.tag_stack.push(tag);
    }

    fn on_end(&mut self, tag_end: TagEnd, range: Range<usize>) {
        self.tag_stack.pop();

        // Handle tag-specific end logic and determine if we need to push a style
        let style = match &tag_end {
            TagEnd::Emphasis => {
                // Reset italic for table cells (no highlight pushed)
                if let Some(ref mut state) = self.table_state {
                    state.cell_italic = false;
                }
                None
            }
            TagEnd::Strong => {
                // Reset bold for table cells (no highlight pushed)
                if let Some(ref mut state) = self.table_state {
                    state.cell_bold = false;
                }
                None
            }
            TagEnd::Strikethrough => None, // No highlight pushed
            TagEnd::CodeBlock => {
                // pulldown synthesizes a block end at end-of-input even for an
                // unterminated fence, so the end event alone does not prove
                // closure. A closing fence always sits after the body, so the
                // block range extends past the body exactly when the fence
                // closed. `take` clears the pending block in either case.
                if let Some(pending) = self.pending_code_block.take()
                    && pending.body_range.end < range.end
                {
                    self.buffers.code_blocks.push(CodeBlockMeta {
                        info: pending.info,
                        body: pending.body_text,
                        body_source_range: pending.body_range,
                    });
                }
                None
            }
            TagEnd::Link | TagEnd::Image => {
                // Clear link state for table cells so subsequent text in
                // the same cell isn't tagged as part of this link.
                if let Some(ref mut state) = self.table_state {
                    state.cell_link = None;
                }
                None
            }
            TagEnd::TableCell => {
                // Finish current cell
                if let Some(ref mut state) = self.table_state {
                    state
                        .current_row
                        .push(std::mem::take(&mut state.current_cell));
                    // Reset cell styles
                    state.cell_bold = false;
                    state.cell_italic = false;
                    state.cell_code = false;
                    state.cell_link = None;
                }
                None
            }
            TagEnd::TableRow => {
                // Finish body row (if not in header)
                if let Some(ref mut state) = self.table_state
                    && !state.in_header
                {
                    let row = std::mem::take(&mut state.current_row);
                    state.rows.push(row);
                }
                None
            }
            TagEnd::TableHead => {
                // Finish header row
                if let Some(ref mut state) = self.table_state {
                    state.header = std::mem::take(&mut state.current_row);
                    state.in_header = false;
                }
                None
            }
            TagEnd::Table => {
                // Finish table: format and store the replacement
                if let Some(mut state) = self.table_state.take() {
                    state.range.end = range.end;
                    let FormattedTable {
                        lines,
                        styled_lines,
                        line_source_offsets,
                        hyperlinks,
                    } = self.format_table(&state);
                    self.buffers.table_replaces.push(TableReplace {
                        lines,
                        styled_lines,
                        range: state.range,
                        line_source_offsets,
                        hyperlinks,
                    });
                }
                None
            }
            _ => None,
        };

        if let Some(style) = style {
            self.push_highlight(Some(style), &range);
        }

        // Track depth and checkpoints
        match &tag_end {
            TagEnd::BlockQuote(_) | TagEnd::List(_) | TagEnd::Item | TagEnd::Table => {
                self.depth = self.depth.saturating_sub(1);
            }
            _ => {}
        }
        if matches!(&tag_end, TagEnd::BlockQuote(_)) {
            self.bq_depth = self.bq_depth.saturating_sub(1);
        }

        // Record checkpoint at depth=0 block boundaries
        if self.depth == 0 {
            let kind = match &tag_end {
                TagEnd::Paragraph => Some(CheckpointKind::Paragraph),
                TagEnd::Heading(_) => Some(CheckpointKind::Heading),
                TagEnd::CodeBlock => Some(CheckpointKind::CodeBlock),
                TagEnd::BlockQuote(_) => Some(CheckpointKind::BlockQuote),
                TagEnd::List(_) => Some(CheckpointKind::List),
                TagEnd::Table => Some(CheckpointKind::Table),
                TagEnd::HtmlBlock => Some(CheckpointKind::HtmlBlock),
                _ => None,
            };

            if let Some(kind) = kind {
                let has_blank = has_blank_line_after(self.text, range.end);
                let is_code_block = matches!(kind, CheckpointKind::CodeBlock);
                let at_eof = range.end >= self.text.len();
                let code_block_properly_closed = is_code_block && !at_eof;

                if has_blank || code_block_properly_closed {
                    // For code blocks, include one newline to properly close the block.
                    // For other blocks (paragraphs, headings, blockquotes, lists),
                    // DON'T include the trailing newline so that when the next chunk
                    // is added, the blank line separator is re-rendered.
                    let checkpoint_pos = if is_code_block && has_blank {
                        range.end + 1
                    } else {
                        range.end
                    };
                    self.last_checkpoint = Some((kind, checkpoint_pos));
                }
            }
        }
    }

    /// Render a mermaid code block into a [`MermaidReplace`]; `true` if drawn.
    fn try_push_mermaid(&mut self, text: &str, range: &Range<usize>) -> bool {
        let line_style = self.ms.rule.style_into();
        let styles = crate::mermaid::MermaidStyles {
            border: line_style,
            node_text: self.ms.text.style_into(),
            edge: line_style,
            edge_label: self.ms.emphasis_inner.style_into(),
            title: self.ms.strong_inner.style_into(),
        };
        match crate::mermaid::render(text, &styles, self.max_table_width) {
            Some(art) => {
                self.buffers
                    .mermaid_replaces
                    .push(crate::buffers::MermaidReplace {
                        lines: art.plain_lines,
                        styled_lines: art.styled_lines,
                        range: range.clone(),
                    });
                true
            }
            None => false,
        }
    }
}
