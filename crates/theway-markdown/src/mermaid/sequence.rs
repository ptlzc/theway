struct Sequence {
    labels: Vec<String>,
    index: HashMap<String, usize>,
    items: Vec<SeqItem>,
}

impl Sequence {
    fn participant(&mut self, id: &str, label: Option<&str>) -> Option<usize> {
        if let Some(&i) = self.index.get(id) {
            if let Some(label) = label {
                self.labels[i] = label.to_string();
            }
            return Some(i);
        }
        if self.labels.len() >= MAX_NODES {
            return None;
        }
        self.index.insert(id.to_string(), self.labels.len());
        self.labels.push(label.unwrap_or(id).to_string());
        Some(self.labels.len() - 1)
    }
}

fn parse_sequence(src: &str) -> Option<Sequence> {
    let mut statements: Vec<String> = Vec::new();
    for raw_line in src.lines() {
        split_statements(raw_line, &mut statements);
    }
    let header = statements.first()?;
    if !header
        .split_whitespace()
        .next()?
        .eq_ignore_ascii_case("sequencediagram")
    {
        return None;
    }

    let mut seq = Sequence {
        labels: Vec::new(),
        index: HashMap::new(),
        items: Vec::new(),
    };
    let mut autonumber = false;
    let mut msg_count = 0usize;
    let mut blocks: Vec<bool> = Vec::new();

    for st in &statements[1..] {
        let first = st.split_whitespace().next().unwrap_or("");
        match first.to_ascii_lowercase().as_str() {
            "participant" | "actor" => {
                let rest = st[first.len()..].trim();
                if rest.is_empty() {
                    return None;
                }
                let (id, label) = match rest.split_once(" as ") {
                    Some((id, label)) => (id.trim(), Some(clean_label(label))),
                    None => (rest, None),
                };
                seq.participant(id, label.as_deref())?;
            }
            "autonumber" => autonumber = true,
            "activate" | "deactivate" | "create" | "destroy" | "title" | "acctitle"
            | "accdescr" | "links" | "link" | "properties" => {}
            "note" => {
                let rest = st[first.len()..].trim();
                let (text_part, anchor) = parse_note_anchor(rest, &mut seq)?;
                if seq.items.len() >= MAX_EDGES {
                    return None;
                }
                seq.items.push(SeqItem::Note {
                    anchor,
                    text: text_part,
                });
            }
            "loop" | "alt" | "opt" | "par" | "critical" | "break" | "else" | "and" | "option" => {
                if matches!(
                    first.to_ascii_lowercase().as_str(),
                    "else" | "and" | "option"
                ) {
                    if blocks.last() != Some(&true) {
                        continue;
                    }
                } else {
                    blocks.push(true);
                }
                if seq.items.len() >= MAX_EDGES {
                    return None;
                }
                seq.items.push(SeqItem::Divider {
                    text: decode_html_entities(st),
                });
            }
            "rect" | "box" => blocks.push(false),
            "end" => {
                if blocks.pop() == Some(true) {
                    if seq.items.len() >= MAX_EDGES {
                        return None;
                    }
                    seq.items.push(SeqItem::Divider {
                        text: "end".to_string(),
                    });
                }
            }
            _ => {
                let (from, to, mut text, dashed, head) = parse_seq_message(st, &mut seq)?;
                if autonumber {
                    msg_count += 1;
                    text = Some(match text {
                        Some(t) => format!("{msg_count}. {t}"),
                        None => format!("{msg_count}."),
                    });
                }
                if seq.items.len() >= MAX_EDGES {
                    return None;
                }
                seq.items.push(SeqItem::Message {
                    from,
                    to,
                    text,
                    dashed,
                    head,
                });
            }
        }
    }

    if seq.labels.is_empty() {
        return None;
    }
    Some(seq)
}

fn parse_note_anchor(rest: &str, seq: &mut Sequence) -> Option<(String, NoteAnchor)> {
    let lower = rest.to_ascii_lowercase();
    let (ids_and_text, kind) = if let Some(r) = lower.strip_prefix("over ") {
        (&rest[rest.len() - r.len()..], 0u8)
    } else if let Some(r) = lower.strip_prefix("left of ") {
        (&rest[rest.len() - r.len()..], 1)
    } else {
        let r = lower.strip_prefix("right of ")?;
        (&rest[rest.len() - r.len()..], 2)
    };
    let (ids, text) = ids_and_text.split_once(':')?;
    let text = decode_html_entities(text.trim());
    let mut parts = ids.split(',').map(str::trim).filter(|s| !s.is_empty());
    let a = seq.participant(parts.next()?, None)?;
    let anchor = match kind {
        0 => {
            let b = match parts.next() {
                Some(id) => seq.participant(id, None)?,
                None => a,
            };
            NoteAnchor::Over(a.min(b), a.max(b))
        }
        1 => NoteAnchor::Left(a),
        _ => NoteAnchor::Right(a),
    };
    Some((text, anchor))
}

fn parse_seq_message(
    st: &str,
    seq: &mut Sequence,
) -> Option<(usize, usize, Option<String>, bool, SeqHead)> {
    let mut found: Option<(usize, &str, bool, SeqHead)> = None;
    for (pos, _) in st.char_indices() {
        for &(op, dashed, head) in SEQ_OPS {
            if st[pos..].starts_with(op) {
                found = Some((pos, op, dashed, head));
                break;
            }
        }
        if found.is_some() {
            break;
        }
    }
    let (pos, op, dashed, head) = found?;
    let from_id = st[..pos].trim();
    if from_id.is_empty() {
        return None;
    }
    let rest = st[pos + op.len()..]
        .trim_start()
        .trim_start_matches(['+', '-']);
    let (to_id, text) = match rest.split_once(':') {
        Some((to, text)) => (to.trim(), non_empty(decode_html_entities(text.trim()))),
        None => (rest.trim(), None),
    };
    if to_id.is_empty() {
        return None;
    }
    let from = seq.participant(from_id, None)?;
    let to = seq.participant(to_id, None)?;
    Some((from, to, text, dashed, head))
}

fn note_geometry(xs: &[usize], anchor: &NoteAnchor, text_w: usize) -> (usize, usize) {
    match *anchor {
        NoteAnchor::Over(l, r) => {
            let center = (xs[l] + xs[r]) / 2;
            let w = (xs[r] - xs[l] + 5).max(text_w + 2 * PAD + 2);
            (center.saturating_sub(w / 2), w)
        }
        NoteAnchor::Left(i) => {
            let w = text_w + 2 * PAD + 2;
            (xs[i].saturating_sub(2 + w - 1), w)
        }
        NoteAnchor::Right(i) => (xs[i] + 2, text_w + 2 * PAD + 2),
    }
}

fn layout_sequence(
    seq: &Sequence,
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let n = seq.labels.len();
    let labels: Vec<String> = seq
        .labels
        .iter()
        .map(|l| fit_label(l, WRAP_WIDTH))
        .collect();
    let box_w: Vec<usize> = labels
        .iter()
        .map(|l| l.width().max(1) + 2 * PAD + 2)
        .collect();
    let box_h = 3usize;

    let item_text_w = |text: &Option<String>| text.as_deref().map(|t| t.width()).unwrap_or(0);

    let mut gaps: Vec<usize> = (0..n.saturating_sub(1))
        .map(|i| SEQ_GAP.max(box_w[i].div_ceil(2) + box_w[i + 1].div_ceil(2) + 1))
        .collect();

    let mut reqs: Vec<(usize, usize, usize)> = Vec::new();
    for item in &seq.items {
        match item {
            SeqItem::Message { from, to, text, .. } => {
                let tw = item_text_w(text);
                if from != to {
                    let (l, r) = (*from.min(to), *from.max(to));
                    reqs.push((l, r, (tw + 2).max(4)));
                } else if *from + 1 < n {
                    reqs.push((*from, *from + 1, 5 + tw + 2));
                }
            }
            SeqItem::Note { anchor, text } => {
                let tw = text.width();
                match *anchor {
                    NoteAnchor::Over(l, r) if l < r => reqs.push((l, r, tw.saturating_sub(1))),
                    NoteAnchor::Over(i, _) => {
                        let half = (tw + 4).div_ceil(2) + 2;
                        if i > 0 {
                            reqs.push((i - 1, i, half));
                        }
                        if i + 1 < n {
                            reqs.push((i, i + 1, half));
                        }
                    }
                    NoteAnchor::Left(i) if i > 0 => reqs.push((i - 1, i, tw + 7)),
                    NoteAnchor::Right(i) if i + 1 < n => reqs.push((i, i + 1, tw + 7)),
                    _ => {}
                }
            }
            SeqItem::Divider { .. } => {}
        }
    }
    reqs.sort_by_key(|&(l, r, _)| r - l);
    for (l, r, need) in reqs {
        let cur: usize = gaps[l..r].iter().sum();
        if cur < need {
            gaps[r - 1] += need - cur;
        }
    }

    let mut xs = vec![0usize; n];
    xs[0] = box_w[0] / 2;
    for i in 1..n {
        xs[i] = xs[i - 1] + gaps[i - 1];
    }

    let mut canvas_w = xs[n - 1] + box_w[n - 1].div_ceil(2) + 1;
    for item in &seq.items {
        match item {
            SeqItem::Message { from, to, text, .. } if from == to => {
                canvas_w = canvas_w.max(xs[*from] + 5 + item_text_w(text) + 1);
            }
            SeqItem::Note { anchor, text } => {
                let (x, w) = note_geometry(&xs, anchor, text.width());
                canvas_w = canvas_w.max(x + w + 1);
            }
            SeqItem::Divider { text } => {
                canvas_w = canvas_w.max(text.width() + 4);
            }
            _ => {}
        }
    }

    let mut rows: Vec<usize> = Vec::with_capacity(seq.items.len());
    let mut y = box_h + 1;
    for item in &seq.items {
        rows.push(y);
        y += match item {
            SeqItem::Message { from, to, text, .. } => {
                if from == to {
                    4
                } else if text.is_some() {
                    3
                } else {
                    2
                }
            }
            SeqItem::Note { .. } => 4,
            SeqItem::Divider { .. } => 2,
        };
    }
    let bottom_top = y;
    let canvas_h = bottom_top + box_h;

    if let Some(mw) = max_width
        && canvas_w > mw
    {
        return Err(Oversize::Width);
    }
    if canvas_w.saturating_mul(canvas_h) > MAX_CANVAS_CELLS {
        return Err(Oversize::Cells);
    }

    let mut canvas = Canvas::new(canvas_w, canvas_h);
    for i in 0..n {
        for by in [0, bottom_top] {
            let p = Placed {
                x: xs[i].saturating_sub(box_w[i] / 2),
                y: by,
                w: box_w[i],
                h: box_h,
                cx: xs[i],
                cy: by + 1,
                rank: 0,
            };
            draw_box(
                &mut canvas,
                &p,
                std::slice::from_ref(&labels[i]),
                Shape::Rect,
            );
        }
    }
    for (item, &r) in seq.items.iter().zip(&rows) {
        if let SeqItem::Note { anchor, text } = item {
            let (x, w) = note_geometry(&xs, anchor, text.width());
            let p = Placed {
                x,
                y: r,
                w,
                h: 3,
                cx: x + w / 2,
                cy: r + 1,
                rank: 0,
            };
            draw_box(&mut canvas, &p, std::slice::from_ref(text), Shape::Rect);
        }
    }
    for &x in &xs {
        canvas.junction(x, box_h - 1, D);
        canvas.seg_v(x, box_h, bottom_top - 1);
        canvas.junction(x, bottom_top, U);
    }

    for (item, &r) in seq.items.iter().zip(&rows) {
        match item {
            SeqItem::Message {
                from,
                to,
                text,
                dashed,
                head,
            } => {
                let line_ch = if *dashed { '╌' } else { '─' };
                if from == to {
                    let x = xs[*from];
                    canvas.junction(x, r, R);
                    canvas.set(x + 1, r, line_ch, Cls::Edge);
                    canvas.set(x + 2, r, line_ch, Cls::Edge);
                    canvas.set(x + 3, r, '╮', Cls::Edge);
                    canvas.set(x + 3, r + 1, '│', Cls::Edge);
                    canvas.set(
                        x + 1,
                        r + 2,
                        if *head == SeqHead::Cross { '×' } else { '◄' },
                        Cls::Edge,
                    );
                    canvas.set(x + 2, r + 2, line_ch, Cls::Edge);
                    canvas.set(x + 3, r + 2, '╯', Cls::Edge);
                    if let Some(t) = text {
                        draw_seq_text(&mut canvas, t, x + 5, r + 1, Cls::Text);
                    }
                } else {
                    let (x0, x1) = (xs[*from], xs[*to]);
                    let rightward = x1 > x0;
                    let arrow_row = if text.is_some() { r + 1 } else { r };
                    let (lo, hi) = (x0.min(x1), x0.max(x1));
                    canvas.junction(x0, arrow_row, if rightward { R } else { L });
                    for x in (lo + 1)..hi {
                        canvas.set(x, arrow_row, line_ch, Cls::Edge);
                    }
                    let head_ch = match (head, rightward) {
                        (SeqHead::Cross, _) => '×',
                        (SeqHead::Arrow, true) => '▶',
                        (SeqHead::Arrow, false) => '◄',
                    };
                    let head_x = if rightward { x1 - 1 } else { x1 + 1 };
                    canvas.set(head_x, arrow_row, head_ch, Cls::Edge);
                    if let Some(t) = text {
                        let span = hi - lo - 1;
                        let t = fit_label(t, span.max(1));
                        let tx = lo + 1 + span.saturating_sub(t.width()) / 2;
                        draw_seq_text(&mut canvas, &t, tx, r, Cls::Text);
                    }
                }
            }
            SeqItem::Note { .. } => {}
            SeqItem::Divider { text } => {
                for x in 0..canvas_w {
                    canvas.set(x, r, '─', Cls::Edge);
                }
                let t = fit_label(text, canvas_w.saturating_sub(4));
                draw_seq_text(&mut canvas, &format!(" {t} "), 2, r, Cls::EdgeLabel);
            }
        }
    }

    canvas.finalize_mask();
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
        fallback: false,
    })
}

fn draw_seq_text(canvas: &mut Canvas, text: &str, x: usize, y: usize, cls: Cls) {
    let mut cur = x;
    for c in text.chars() {
        let cw = char_width(c).max(1);
        for k in 0..cw {
            if cur + k < canvas.w && y < canvas.h {
                let i = canvas.idx(cur + k, y);
                canvas.mask[i] = 0;
            }
            canvas.set(cur + k, y, if k == 0 { c } else { CONT }, cls);
        }
        cur += cw;
    }
}

const TOO_WIDE_HINT: &str =
    "This diagram is too wide to display here \u{2014} open the image to view it in full.";

fn fallback(
    src: &str,
    styles: &MermaidStyles,
    max_width: Option<usize>,
    too_wide: bool,
) -> MermaidArt {
    let header = first_word(src);
    let title = format!(" mermaid: {header} ");
    let limit = max_width.map(|m| m.saturating_sub(4).max(8));
    let body: Vec<String> = src
        .lines()
        .map(|l| l.trim_end())
        .skip_while(|l| l.is_empty())
        .flat_map(|l| chunk_line(l, limit))
        .collect();
    let content_w = body
        .iter()
        .map(|l| l.width())
        .chain(std::iter::once(title.width()))
        .max()
        .unwrap_or(0);
    let inner = content_w + 2;

    let mut styled = Vec::new();
    let mut plain = Vec::new();

    let mut top = String::from("╭");
    top.push_str(&title);
    for _ in 0..inner.saturating_sub(title.width()) {
        top.push('─');
    }
    top.push('╮');
    styled.push(Line::from(vec![
        Span::styled("╭".to_string(), styles.border),
        Span::styled(title.clone(), styles.title),
        Span::styled(
            format!("{}╮", "─".repeat(inner.saturating_sub(title.width()))),
            styles.border,
        ),
    ]));
    plain.push(top);

    for line in &body {
        let pad = content_w.saturating_sub(line.width());
        styled.push(Line::from(vec![
            Span::styled("│ ".to_string(), styles.border),
            Span::styled(line.clone(), styles.node_text),
            Span::styled(format!("{} │", " ".repeat(pad)), styles.border),
        ]));
        plain.push(format!("│ {}{} │", line, " ".repeat(pad)));
    }

    let bottom = format!("╰{}╯", "─".repeat(inner));
    styled.push(Line::from(Span::styled(bottom.clone(), styles.border)));
    plain.push(bottom);

    if too_wide {
        let hint_style = styles.border.add_modifier(Modifier::ITALIC);
        for chunk in wrap_words(TOO_WIDE_HINT, max_width) {
            styled.push(Line::from(Span::styled(chunk.clone(), hint_style)));
            plain.push(chunk);
        }
    }

    MermaidArt {
        styled_lines: styled,
        plain_lines: plain,
        fallback: true,
    }
}

fn chunk_line(line: &str, limit: Option<usize>) -> Vec<String> {
    let Some(limit) = limit else {
        return vec![line.to_string()];
    };
    if line.width() <= limit {
        return vec![line.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for c in line.chars() {
        let cw = char_width(c).max(1);
        if cur_w + cw > limit && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(c);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn wrap_words(text: &str, limit: Option<usize>) -> Vec<String> {
    let Some(limit) = limit else {
        return vec![text.to_string()];
    };
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if cur.is_empty() {
            cur.push_str(word);
        } else if cur.width() + 1 + word.width() <= limit {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
        .into_iter()
        .flat_map(|l| chunk_line(&l, Some(limit)))
        .collect()
}
