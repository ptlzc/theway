impl<'a, 'b, 'syn, 'oc> MarkdownParser<'a, 'b, 'syn, 'oc> {
    pub fn new(
        text: &'a str,
        ms: MarkdownStyle,
        buffers: &'b mut MarkdownBuffers,
        syntect: Option<&'syn Syntect>,
    ) -> Self {
        Self {
            text,
            ms,
            buffers,
            syntect,
            open_code: None,
            tag_stack: Vec::new(),
            table_state: None,
            depth: 0,
            bq_depth: 0,
            last_checkpoint: None,
            max_table_width: None,
            link_id_counter: 0,
            collapse_soft_breaks: true,
            pending_code_block: None,
        }
    }

    /// Set whether CommonMark soft breaks collapse to a space.
    ///
    /// Defaults to `true`. Set `false` for source-faithful rendering (plan
    /// preview) where each source line must keep its own visual line and
    /// `line_source_map` entry.
    pub fn collapse_soft_breaks(mut self, collapse: bool) -> Self {
        self.collapse_soft_breaks = collapse;
        self
    }

    /// Set the maximum width for rendered tables.
    ///
    /// When set, column widths are shrunk proportionally so the table
    /// fits within the given number of display columns.
    pub fn max_table_width(mut self, width: Option<usize>) -> Self {
        self.max_table_width = width;
        self
    }

    /// Set the starting link ID counter (for streaming renderer continuity).
    ///
    /// Internal: only the in-crate streaming renderer needs to manage the
    /// link counter across `rerender_tail` calls.  Consumers should use
    /// `StreamingMarkdownRenderer` instead of touching the parser directly.
    pub(crate) fn link_id_start(mut self, id: u32) -> Self {
        self.link_id_counter = id;
        self
    }

    /// Provide an incremental highlighter for the trailing still-open fenced
    /// code block (streaming tail re-render only).
    ///
    /// Internal: lets `rerender_tail` persist syntect's resumable per-line state
    /// across passes so an open code block is highlighted in O(N) total instead
    /// of O(N²). Batch/non-streaming callers leave this `None`.
    pub(crate) fn open_code(mut self, cache: Option<&'oc mut OpenCodeHighlighter>) -> Self {
        self.open_code = cache;
        self
    }

    /// Parse markdown and return a ParsedMarkdown ready for rendering.
    ///
    /// Consumes self, dropping transient parsing state.
    pub fn parse(mut self) -> ParsedMarkdown<'a, 'b> {
        self.tag_stack.clear();
        self.buffers.clear();
        self.table_state = None;
        self.depth = 0;
        self.last_checkpoint = None;
        self.pending_code_block = None;

        for (event, range) in
            TextMergeWithOffset::new(theway_markdown_core::offset_events(self.text))
        {
            self.on_event(event, range);
        }

        ParsedMarkdown::new(
            self.text,
            self.ms,
            self.buffers,
            self.last_checkpoint,
            self.link_id_counter,
            self.syntect
                .map_or_else(get_color_level, Syntect::color_level),
        )
    }

    fn push_highlight(&mut self, style: Option<Style>, range: &Range<usize>) {
        self.buffers.highlights.push(Highlight {
            style,
            range: range.clone(),
        });
    }

    fn on_event(&mut self, event: Event<'a>, range: Range<usize>) {
        let mut parent_code_block = None;

        // Apply ALL ancestors' inner styles to non-marker events.
        let skip_inner_style = matches!(
            event,
            Event::Start(_) | Event::End(_) | Event::Code(_) | Event::InlineMath(_)
        );

        // Collect ancestor styles first (to avoid borrow issues)
        let ancestor_styles: Vec<Option<Style>> = if !skip_inner_style {
            // Inside a link, inline-format ancestors (strong/emphasis/
            // strikethrough) must not recolor the link text: their inner
            // styles carry the theme's default text fg, and these highlights
            // land *after* the link_text highlight pushed at Tag::Link start
            // — merge_styles is last-wins on fg, so keeping the fg would
            // clobber the link color (e.g. `**[bold link](url)**`). Only the
            // fg competes with link_text today, so effects (and any bg) pass
            // through.
            let in_link = self
                .tag_stack
                .iter()
                .any(|t| matches!(t, Tag::Link { .. } | Tag::Image { .. }));
            let strip_fg_in_link =
                |style: Style| if in_link { style.fg_color(None) } else { style };
            self.tag_stack
                .iter()
                .filter_map(|ancestor| match ancestor {
                    Tag::Heading { level, .. } => {
                        Some(Some(self.ms.heading_inner[(*level as i32) as usize - 1]))
                    }
                    Tag::Emphasis => Some(Some(strip_fg_in_link(self.ms.emphasis_inner))),
                    Tag::Strong => Some(Some(strip_fg_in_link(self.ms.strong_inner))),
                    Tag::Strikethrough => Some(Some(strip_fg_in_link(self.ms.strikethrough_inner))),
                    // Link/Image already push their own inner-style highlight
                    // (link_text) during on_start.  We just need ancestor_styles
                    // to be non-empty so the Event::Text branch below skips
                    // pushing ms.text — which would otherwise override the
                    // link_text foreground color via merge_styles' last-wins
                    // ordering.
                    Tag::Link { .. } | Tag::Image { .. } => Some(None),
                    Tag::CodeBlock(block) => {
                        parent_code_block = Some(match block {
                            CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                                Some(lang.to_owned())
                            }
                            _ => None,
                        });
                        None
                    }
                    _ => None,
                })
                .collect()
        } else {
            if let Some(Tag::CodeBlock(block)) = self.tag_stack.last() {
                parent_code_block = Some(match block {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_owned()),
                    _ => None,
                });
            }
            Vec::new()
        };

        for style in &ancestor_styles {
            self.push_highlight(*style, &range);
        }

        match event {
            Event::Start(tag) => self.on_start(tag, range),
            Event::End(tag_end) => self.on_end(tag_end, range),
            Event::Text(text) => {
                // Capture text into table cell if we're inside a table
                if let Some(ref mut state) = self.table_state {
                    state.push_text(&text);
                }

                // Record the enclosing fenced block's raw byte range and its
                // de-prefixed body content. pulldown merges the body into one
                // text event, but accumulate defensively in case it is split.
                if parent_code_block.is_some()
                    && let Some(pending) = self.pending_code_block.as_mut()
                {
                    if pending.body_seen {
                        pending.body_range.start = pending.body_range.start.min(range.start);
                        pending.body_range.end = pending.body_range.end.max(range.end);
                    } else {
                        pending.body_range = range.clone();
                        pending.body_seen = true;
                    }
                    pending.body_text.push_str(&text);
                }

                if let Some(parent_code_block) = parent_code_block {
                    // Closed mermaid fences render as a diagram; open ones fall
                    // through so the source shows while still streaming.
                    if let Some(lang) = parent_code_block.as_deref()
                        && lang
                            .split_whitespace()
                            .next()
                            .is_some_and(|t| t.eq_ignore_ascii_case("mermaid"))
                        && range.end < self.text.len()
                        && self.try_push_mermaid(&text, &range)
                    {
                        return;
                    }
                    let highlighted = match parent_code_block {
                        Some(lang) => {
                            if let Some(syn) = self.syntect
                                && let Some(cache) = self.open_code.as_deref_mut()
                            {
                                // Streaming tail: the cache routes between the
                                // incremental open-block path and the
                                // closed-fence memo.
                                cache.highlight_block(
                                    syn,
                                    &lang,
                                    range.start,
                                    range.end >= self.text.len(),
                                    &text,
                                )
                            } else {
                                // Batch render (no streaming caches attached).
                                syntax_highlight_raw(self.syntect, &lang, &text)
                            }
                        }
                        None => None,
                    };
                    if let Some(highlighted) = highlighted {
                        self.buffers.replaces.push(Replace {
                            highlighted,
                            range: range.clone(),
                        });
                    } else {
                        self.push_highlight(Some(self.ms.code_untagged), &range);
                        self.buffers.untagged_code_ranges.push(range.clone());
                    }
                } else {
                    if ancestor_styles.is_empty() {
                        // Apply the default text style only when no ancestor
                        // (heading, strong, emphasis, etc.) already provides a
                        // color — otherwise ms.text would override them.
                        self.push_highlight(Some(self.ms.text), &range);
                    } else {
                        self.push_highlight(None, &range);
                    }
                    if self.table_state.is_none() {
                        self.scan_inline_html_entities(&range);
                    }
                }
            }
            Event::Code(code) => {
                // Capture code content into table cell if we're inside a table
                if let Some(ref mut state) = self.table_state {
                    let prev_code = state.cell_code;
                    state.cell_code = true;
                    state.push_text(&code);
                    state.cell_code = prev_code;
                }
                self.style_inline_code_span(&code, &range);
            }
            Event::InlineMath(math) => {
                // `$...$` inline math: render the TeX to Unicode and swap it
                // in via a pretty-mode transform. Falls back to inline-code
                // presentation when conversion declines (oversized input) or
                // produces nothing visible.
                let rendered = latex::latex_to_unicode_inline(&math).filter(|r| !r.is_empty());

                if let Some(ref mut state) = self.table_state {
                    match &rendered {
                        Some(r) => {
                            let prev_italic = state.cell_italic;
                            state.cell_italic = true;
                            state.push_text(r);
                            state.cell_italic = prev_italic;
                        }
                        None => {
                            let prev_code = state.cell_code;
                            state.cell_code = true;
                            state.push_text(&math);
                            state.cell_code = prev_code;
                        }
                    }
                }

                match rendered {
                    Some(r) => {
                        // One highlight + one transform spanning the entire
                        // `$...$` range: pretty mode shows the rendered math,
                        // raw mode shows the TeX source in the math style.
                        self.push_highlight(Some(self.ms.math), &range);
                        self.buffers.transforms.push(Transform {
                            range: range.clone(),
                            to: r,
                            force: false,
                        });
                    }
                    None => self.style_inline_code_span(&math, &range),
                }
            }
            Event::SoftBreak => {
                // Collapse soft breaks to spaces unless the next source
                // byte is a list-item indent or blockquote `>` marker
                // (the byte immediately after pulldown's SoftBreak range),
                // in which case the line ending belongs to a block
                // continuation and the renderer surfaces it as its own
                // visual line. The transform spans the full range so CRLF
                // (`\r\n`, 2 bytes) preserves byte length.
                if let Some(ref mut state) = self.table_state {
                    state.push_text(" ");
                } else {
                    let next = self.text.as_bytes().get(range.end);
                    let is_continuation = matches!(next, Some(b' ' | b'\t' | b'>' | b'|'));
                    if self.collapse_soft_breaks && !is_continuation {
                        let span = range.end - range.start;
                        debug_assert!(span >= 1, "SoftBreak range must cover at least one byte");
                        self.buffers.transforms.push(Transform {
                            range: range.clone(),
                            to: " ".repeat(span),
                            force: true,
                        });
                    }
                }
                self.push_highlight(None, &range);
            }
            Event::HardBreak => {
                if let Some(ref mut state) = self.table_state {
                    state.push_text("\n");
                }
                self.push_highlight(None, &range);
            }
            Event::Html(_) => {
                // Render HTML block content as regular text (not code).
                // pulldown-cmark treats XML-like tags (e.g. <example>) as HTML
                // blocks, which previously got code-block styling via Replace.
                self.push_highlight(Some(self.ms.text), &range);
            }
            Event::InlineHtml(html) => {
                if is_br_tag(&html) {
                    if let Some(ref mut state) = self.table_state {
                        state.push_text("\n");
                        self.push_highlight(Some(self.ms.text), &range);
                    } else {
                        self.buffers.transforms.push(Transform {
                            range: range.clone(),
                            to: "\n".to_string(),
                            force: false,
                        });
                        self.push_highlight(None, &range);
                    }
                } else if let Some(ref mut state) = self.table_state {
                    state.push_text(&html);
                    self.push_highlight(Some(self.ms.text), &range);
                } else if let Some(highlighted) = syntax_highlight_raw(self.syntect, "html", &html)
                {
                    self.buffers.replaces.push(Replace {
                        highlighted,
                        range: range.clone(),
                    });
                }
            }
            Event::DisplayMath(math) => {
                // `$$...$$` display math: render to Unicode block lines.
                if let Some(ref mut state) = self.table_state {
                    // Inside a table cell there is no room for a block:
                    // render single-line (rows joined with `; `).
                    match latex::latex_to_unicode_inline(&math).filter(|r| !r.is_empty()) {
                        Some(r) => {
                            let prev_italic = state.cell_italic;
                            state.cell_italic = true;
                            state.push_text(&r);
                            state.cell_italic = prev_italic;
                        }
                        None => {
                            let prev_code = state.cell_code;
                            state.cell_code = true;
                            state.push_text(&math);
                            state.cell_code = prev_code;
                        }
                    }
                    self.push_highlight(Some(self.ms.math), &range);
                } else if self.push_display_math_block(range.clone(), &math) {
                    // Raw mode shows the TeX source in the math style; pretty
                    // mode consumes the range via the block replacement.
                    self.push_highlight(Some(self.ms.math), &range);
                } else {
                    // Fallback (conversion declined / nothing visible):
                    // legacy presentation — TeX source highlighted as code.
                    self.push_highlight(Some(self.ms.code_outer), &range);
                    let outer_text = &self.text[range.clone()];
                    if let Some(r) = find_substring(outer_text, &math, true, false) {
                        let inner_range = (range.start + r.start)..(range.start + r.end);
                        if let Some(highlighted) = syntax_highlight_raw(self.syntect, "tex", &math)
                        {
                            self.buffers.replaces.push(Replace {
                                highlighted,
                                range: inner_range,
                            });
                        } else {
                            self.push_highlight(Some(self.ms.code_untagged), &inner_range);
                        }
                    }
                }
            }
            Event::FootnoteReference(_) => {
                self.push_highlight(Some(self.ms.link_outer), &range);
            }
            Event::Rule => {
                // Style and transform "---" to "───" (horizontal rule)
                self.push_highlight(Some(self.ms.rule), &range);
                let rule_text = &self.text[range.clone()];
                if let Some(marker_end) = rule_text.find('\n') {
                    // Transform only up to the newline
                    self.buffers.transforms.push(Transform {
                        range: range.start..range.start + marker_end,
                        to: "───".to_string(),
                        force: false,
                    });
                } else {
                    // No trailing newline, transform the whole range
                    self.buffers.transforms.push(Transform {
                        range: range.clone(),
                        to: "───".to_string(),
                        force: false,
                    });
                }
                if self.depth == 0 {
                    self.last_checkpoint = Some((CheckpointKind::ThematicBreak, range.end));
                }
            }
            Event::TaskListMarker(checked) => {
                let style = if checked {
                    self.ms.task_checked
                } else {
                    self.ms.task_unchecked
                };
                self.push_highlight(Some(style), &range);
            }
        }
    }
}
