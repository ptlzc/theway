impl<'a, 'b, 'syn, 'oc> MarkdownParser<'a, 'b, 'syn, 'oc> {
    /// Apply inline-code styling to a code/math span: dim the delimiters,
    /// style the content. Shared by `Event::Code` and the inline-math
    /// fallback path.
    fn style_inline_code_span(&mut self, code: &CowStr<'_>, range: &Range<usize>) {
        // Find the actual content range (excluding the delimiters).
        let outer_text = &self.text[range.clone()];
        if let Some(inner_range) = find_substring(outer_text, code, false, false)
            .or_else(|| find_substring(outer_text, code, true, false))
        {
            let absolute_inner = (range.start + inner_range.start)..(range.start + inner_range.end);

            // Left delimiter
            if inner_range.start > 0 {
                self.push_highlight(
                    Some(self.ms.inline_code_outer),
                    &(range.start..absolute_inner.start),
                );
            }
            // Inner code
            self.push_highlight(Some(self.ms.inline_code_inner), &absolute_inner);
            // Right delimiter
            if range.end > absolute_inner.end {
                self.push_highlight(
                    Some(self.ms.inline_code_outer),
                    &(absolute_inner.end..range.end),
                );
            }
        } else {
            self.push_highlight(Some(self.ms.inline_code_inner), range);
        }
    }

    /// Scan a prose `Event::Text` source range for HTML character entity
    /// references (`&lt;`, `&gt;`, `&amp;`, numeric, …) and decode each via a
    /// pretty-mode transform, so e.g. `&lt;` displays as `<`.
    ///
    /// The source-faithful renderer renders the raw source bytes for prose,
    /// which would otherwise leave entities undecoded (table cells already
    /// decode through the cell-text path at `Event::Text` → `push_text`). The
    /// transform is non-`force`, so raw mode still shows the verbatim source.
    ///
    /// A `None`-style highlight is pushed over each entity's byte range so the
    /// renderer splits a chunk exactly there: this keeps the substitution from
    /// straddling a chunk boundary (which would emit the replacement twice)
    /// while leaving the surrounding text/ancestor styling untouched. Code
    /// spans and fenced blocks never reach here, so entities inside code stay
    /// literal.
    ///
    /// Panic-safety: pulldown-cmark guarantees `range` is a valid sub-slice
    /// of `self.text`; even so, the access goes through `str::get` and
    /// `slice::get` so a future invariant violation degrades to a no-op rather
    /// than panicking. The inner loop only advances over ASCII bytes
    /// (`#`/`a-z`/`A-Z`/`0-9`/`;`), guaranteeing `i` and `end` stay on UTF-8
    /// char boundaries.
    fn scan_inline_html_entities(&mut self, range: &Range<usize>) {
        let Some(slice) = self.text.get(range.clone()) else {
            debug_assert!(false, "pulldown-cmark text range out of bounds");
            return;
        };
        if !slice.contains('&') {
            return;
        }
        // Longest HTML5 named entity reference (`&CounterClockwiseContourIntegral;`)
        // is 33 bytes including the leading `&` and trailing `;`. Bounding the
        // scan keeps a run of bare `&` characters from degrading to O(n²).
        const MAX_ENTITY_LEN: usize = 33;
        let bytes = slice.as_bytes();
        let mut i = 0;
        while let Some(&b) = bytes.get(i) {
            if b != b'&' {
                i += 1;
                continue;
            }
            // An entity reference contains only ASCII name/numeric characters
            // and no internal `;`, so the first `;` reached while consuming
            // valid characters closes it. Stopping on any other byte avoids
            // both quadratic scans and slicing through a multi-byte char.
            let max = (i + MAX_ENTITY_LEN).min(bytes.len());
            let mut j = i + 1;
            let end = loop {
                if j >= max {
                    break None;
                }
                match bytes.get(j) {
                    Some(b';') => break Some(j),
                    Some(b'#' | b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9') => j += 1,
                    _ => break None,
                }
            };
            if let Some(end) = end
                && let Some(entity) = slice.get(i..=end)
                && let Some(decoded) = decode_html_entity(entity)
            {
                let abs = (range.start + i)..(range.start + end + 1);
                // An earlier scan (e.g. `\(...\)` math) may have already claimed
                // these bytes with its own transform. Overlapping transforms
                // would each emit their replacement, so leave the entity to the
                // existing transform rather than double-substituting.
                let overlaps = self
                    .buffers
                    .transforms
                    .iter()
                    .any(|t| t.range.start < abs.end && abs.start < t.range.end);
                if !overlaps {
                    self.push_highlight(None, &abs);
                    self.buffers.transforms.push(Transform {
                        range: abs,
                        to: decoded,
                        force: false,
                    });
                }
                i = end + 1;
                continue;
            }
            i += 1;
        }
    }

    /// Push a pretty-mode block replacement rendering `latex_src` as display
    /// math over `range`. Returns `false` when conversion declines
    /// (oversized input) or produces nothing visible; callers then fall back
    /// to a raw presentation.
    ///
    /// Reuses the table block-replacement machinery: pre-rendered styled
    /// lines that substitute the source range in pretty mode only, so raw
    /// mode keeps showing the TeX source.
    fn push_display_math_block(&mut self, range: Range<usize>, latex_src: &str) -> bool {
        let Some(rendered) = latex::latex_to_unicode_display(latex_src) else {
            return false;
        };
        if rendered.is_empty() {
            return false;
        }
        // Consume the line ending right after the closing delimiter, like
        // table ranges do. Without this, a batch render emits an extra blank
        // line after the block (the source newline) that the streaming
        // checkpoint+tail path does not, breaking render convergence.
        let mut range = range;
        if self.text[range.end..].starts_with("\r\n") {
            range.end += 2;
        } else if self.text[range.end..].starts_with('\n') {
            range.end += 1;
        }
        let style: ratatui::style::Style = self.ms.math.style_into();
        let src_newlines = self.text[range.clone()]
            .bytes()
            .filter(|&b| b == b'\n')
            .count();
        let mut lines = Vec::with_capacity(rendered.len());
        let mut styled_lines = Vec::with_capacity(rendered.len());
        let mut line_source_offsets = Vec::with_capacity(rendered.len());
        for (i, line) in rendered.iter().enumerate() {
            let text = format!("  {line}");
            styled_lines.push(Line::from(Span::styled(text.clone(), style)));
            lines.push(text);
            // Best-effort scroll mapping: the i-th rendered line maps to the
            // i-th content line of the block (clamped to its source lines).
            line_source_offsets.push((i + 1).min(src_newlines));
        }
        self.buffers.table_replaces.push(TableReplace {
            lines,
            styled_lines,
            range,
            line_source_offsets,
            hyperlinks: Vec::new(),
        });
        true
    }
}
