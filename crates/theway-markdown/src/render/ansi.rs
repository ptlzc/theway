impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    /// Render to ANSI-styled string.
    ///
    /// If `pretty` is true, syntax markers are hidden.
    /// Returns the rendered string and a source map for copy-paste support.
    pub fn render_ansi(&mut self, pretty: bool) -> (String, SourceMap) {
        let events = self.build_render_events();

        // Apply force transforms in place over a copy of `self.text` so
        // the ANSI path picks them up without restructuring `push`. See
        // `Transform::force` for the byte-length invariant.
        let text_owned: Option<String> = if self.buffers.transforms.iter().any(|t| t.force) {
            let mut bytes = self.text.as_bytes().to_vec();
            for t in &self.buffers.transforms {
                if !t.force {
                    continue;
                }
                debug_assert_eq!(
                    t.to.len(),
                    t.range.end - t.range.start,
                    "force transforms must preserve byte length",
                );
                debug_assert!(
                    self.text.is_char_boundary(t.range.start)
                        && self.text.is_char_boundary(t.range.end),
                    "force transform range must align with char boundaries",
                );
                bytes[t.range.clone()].copy_from_slice(t.to.as_bytes());
            }
            Some(String::from_utf8(bytes).expect("force transforms preserve UTF-8"))
        } else {
            None
        };
        let view_text: &str = text_owned.as_deref().unwrap_or(self.text);

        let mut out = String::with_capacity(view_text.len() * 2);
        let mut source_map = SourceMap::new();
        let mut rendered_offset = 0;
        let mut hl_ids = BTreeSet::<usize>::new();
        let mut last_pos = 0;
        let mut replace: Option<usize> = None;
        let mut table_replace: Option<usize> = None;
        let mut mermaid_replace: Option<usize> = None;
        let mut current = (0..0, Style::new());

        fn push(
            out: &mut String,
            current: &mut (Range<usize>, Style),
            text: &str,
            range: Range<usize>,
            style: Style,
            source_map: &mut SourceMap,
            rendered_offset: &mut usize,
        ) {
            let (crange, cstyle) = current;
            let ctext = &text[crange.clone()];
            if !range.is_empty() && style == *cstyle {
                if !ctext.is_empty() {
                    debug_assert_eq!(crange.end, range.start);
                }
                crange.end = range.end;
                return;
            }
            if !ctext.is_empty() {
                source_map.add(*rendered_offset, crange.clone());
                *rendered_offset += ctext.len();

                if cstyle.is_plain() {
                    out.push_str(ctext);
                } else {
                    out.push_str(&ctext.astyle(*cstyle).to_string());
                }
            }
            *crange = range;
            *cstyle = style;
        }

        for ev in &events {
            if replace.is_none()
                && table_replace.is_none()
                && mermaid_replace.is_none()
                && ev.pos > last_pos
            {
                let should_skip =
                    pretty && all_hidden(hl_ids.iter().map(|&i| self.buffers.highlights[i].style));

                if should_skip {
                    push(
                        &mut out,
                        &mut current,
                        view_text,
                        ev.pos..ev.pos,
                        Style::new(),
                        &mut source_map,
                        &mut rendered_offset,
                    );
                } else {
                    let mut style =
                        merge_styles(hl_ids.iter().map(|&i| self.buffers.highlights[i].style));
                    let text = &view_text[last_pos..ev.pos];
                    let is_invert = style.get_effects().contains(Effects::INVERT);
                    if text.as_bytes().iter().all(|&ch| ch == b'\n')
                        || (text.as_bytes().iter().all(u8::is_ascii_whitespace)
                            && ((!is_invert && style.get_bg_color().is_none())
                                || (is_invert && style.get_fg_color().is_none())))
                    {
                        style = Style::new();
                    }
                    push(
                        &mut out,
                        &mut current,
                        view_text,
                        last_pos..ev.pos,
                        style,
                        &mut source_map,
                        &mut rendered_offset,
                    );
                }
                last_pos = ev.pos;
            }

            match ev.kind {
                RenderEventKind::Replace => {
                    if ev.is_end && replace == Some(ev.index) {
                        replace = None;
                        out.push_str(&Reset.to_string());
                    } else if !ev.is_end && replace.is_none() && table_replace.is_none() {
                        replace = Some(ev.index);
                        push(
                            &mut out,
                            &mut current,
                            view_text,
                            ev.pos..ev.pos,
                            Style::new(),
                            &mut source_map,
                            &mut rendered_offset,
                        );
                        out.push_str(&Reset.to_string());

                        let repl = &self.buffers.replaces[ev.index];
                        let ansi_content =
                            render_replace_ansi(&repl.highlighted, self.color_level);

                        let replace_text_len: usize = repl
                            .highlighted
                            .iter()
                            .flat_map(|line| line.iter().map(|(_, t)| t.len()))
                            .sum();
                        source_map.add(rendered_offset, repl.range.clone());
                        rendered_offset += replace_text_len;

                        out.push_str(&ansi_content);
                        last_pos = repl.range.end;
                    }
                }
                RenderEventKind::Table => {
                    if ev.is_end && table_replace == Some(ev.index) {
                        table_replace = None;
                    } else if !ev.is_end && table_replace.is_none() && pretty {
                        table_replace = Some(ev.index);
                        push(
                            &mut out,
                            &mut current,
                            view_text,
                            ev.pos..ev.pos,
                            Style::new(),
                            &mut source_map,
                            &mut rendered_offset,
                        );

                        let trepl = &self.buffers.table_replaces[ev.index];
                        // Block lines must start at a line boundary; a
                        // display-math replacement can occur mid-paragraph.
                        // Styled chunks end with a reset sequence after the
                        // newline, so check both forms.
                        let at_line_start =
                            out.is_empty() || out.ends_with('\n') || out.ends_with("\n\x1b[0m");
                        if !at_line_start {
                            out.push('\n');
                            rendered_offset += 1;
                        }
                        for line in &trepl.lines {
                            out.push_str(line);
                            out.push('\n');
                            rendered_offset += line.len() + 1;
                        }
                        last_pos = trepl.range.end;
                        // Advance `current` past the table so trailing text
                        // doesn't merge back to the pre-table position.
                        current.0 = trepl.range.end..trepl.range.end;
                    }
                }
                RenderEventKind::Mermaid => {
                    if ev.is_end && mermaid_replace == Some(ev.index) {
                        mermaid_replace = None;
                    } else if !ev.is_end && mermaid_replace.is_none() && pretty {
                        mermaid_replace = Some(ev.index);
                        push(
                            &mut out,
                            &mut current,
                            view_text,
                            ev.pos..ev.pos,
                            Style::new(),
                            &mut source_map,
                            &mut rendered_offset,
                        );

                        let mrepl = &self.buffers.mermaid_replaces[ev.index];
                        for line in &mrepl.lines {
                            out.push_str(line);
                            out.push('\n');
                            rendered_offset += line.len() + 1;
                        }
                        last_pos = mrepl.range.end;
                        current.0 = mrepl.range.end..mrepl.range.end;
                    }
                }
                RenderEventKind::Highlight => {
                    if ev.is_end {
                        hl_ids.remove(&ev.index);
                    } else {
                        hl_ids.insert(ev.index);
                    }
                }
            }
        }

        let len = view_text.len();
        if last_pos < len {
            push(
                &mut out,
                &mut current,
                view_text,
                last_pos..len,
                Style::new(),
                &mut source_map,
                &mut rendered_offset,
            );
        }
        push(
            &mut out,
            &mut current,
            view_text,
            len..len,
            Style::new(),
            &mut source_map,
            &mut rendered_offset,
        );
        (out, source_map)
    }
}
