fn assign_tracks(spans: &[(usize, usize, usize, usize, usize)]) -> (Vec<(usize, usize)>, usize) {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut tracks: Vec<Vec<(usize, usize, usize, usize)>> = Vec::new();
    let mut out = Vec::with_capacity(sorted.len());
    for &(s, e, f, t, idx) in &sorted {
        let compatible = |members: &Vec<(usize, usize, usize, usize)>| {
            members
                .iter()
                .all(|&(s2, e2, f2, t2)| e2 + 2 <= s || e + 2 <= s2 || f2 == f || t2 == t)
        };
        let slot = match tracks.iter().position(compatible) {
            Some(x) => x,
            None => {
                tracks.push(Vec::new());
                tracks.len() - 1
            }
        };
        tracks[slot].push((s, e, f, t));
        out.push((idx, slot));
    }
    (out, tracks.len())
}

/// Reorder nodes within each rank to minimize edge crossings (Sugiyama-style
/// barycenter sweeps): alternate down/up passes sort each rank by the mean
/// position of its forward neighbours, keeping the ordering with the fewest
/// crossings between adjacent ranks.
fn order_ranks(by_rank: &mut [Vec<usize>], edges: &[Edge], ranks: &[usize]) {
    let n = ranks.len();
    if by_rank.len() < 2 || n < 3 {
        return;
    }
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if e.from != e.to && ranks[e.to] > ranks[e.from] {
            parents[e.to].push(e.from);
            children[e.from].push(e.to);
        }
    }

    let mut pos = vec![0usize; n];
    let set_pos = |by_rank: &[Vec<usize>], pos: &mut Vec<usize>| {
        for row in by_rank {
            for (i, &v) in row.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    set_pos(by_rank, &mut pos);

    let mut best: Vec<Vec<usize>> = by_rank.to_vec();
    let mut best_crossings = count_crossings(edges, ranks, &pos);
    if best_crossings == 0 {
        return;
    }

    for it in 0..8 {
        if it % 2 == 0 {
            for row in by_rank.iter_mut().skip(1) {
                sort_by_barycenter(row, &parents, &pos);
                for (i, &v) in row.iter().enumerate() {
                    pos[v] = i;
                }
            }
        } else {
            let last = by_rank.len() - 1;
            for row in by_rank[..last].iter_mut().rev() {
                sort_by_barycenter(row, &children, &pos);
                for (i, &v) in row.iter().enumerate() {
                    pos[v] = i;
                }
            }
        }
        let crossings = count_crossings(edges, ranks, &pos);
        if crossings < best_crossings {
            best_crossings = crossings;
            best = by_rank.to_vec();
        }
        if best_crossings == 0 {
            break;
        }
    }

    for (row, b) in by_rank.iter_mut().zip(best) {
        *row = b;
    }
}

fn sort_by_barycenter(row: &mut [usize], neigh: &[Vec<usize>], pos: &[usize]) {
    let mut keyed: Vec<(f64, usize)> = row
        .iter()
        .map(|&v| {
            let key = if neigh[v].is_empty() {
                pos[v] as f64
            } else {
                neigh[v].iter().map(|&u| pos[u] as f64).sum::<f64>() / neigh[v].len() as f64
            };
            (key, v)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (slot, (_, v)) in row.iter_mut().zip(keyed) {
        *slot = v;
    }
}

fn count_crossings(edges: &[Edge], ranks: &[usize], pos: &[usize]) -> usize {
    let adjacent: Vec<(usize, usize, usize)> = edges
        .iter()
        .filter(|e| e.from != e.to && ranks[e.to] == ranks[e.from] + 1)
        .map(|e| (ranks[e.from], pos[e.from], pos[e.to]))
        .collect();
    let mut crossings = 0;
    for (i, a) in adjacent.iter().enumerate() {
        for b in &adjacent[i + 1..] {
            if a.0 == b.0 && ((a.1 < b.1 && a.2 > b.2) || (a.1 > b.1 && a.2 < b.2)) {
                crossings += 1;
            }
        }
    }
    crossings
}

/// Assign a center coordinate (along the cross-axis) to every node so nodes line
/// up under their neighbours. Iterative barycenter relaxation: each node drifts
/// toward the average of its forward neighbours while ranks keep order and a
/// minimum `sep` between boxes, which straightens chains and centers branches.
fn assign_positions(
    by_rank: &[Vec<usize>],
    size: &[usize],
    sep: usize,
    edges: &[Edge],
    ranks: &[usize],
) -> Vec<usize> {
    let n = size.len();
    let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in edges {
        if e.from != e.to && ranks[e.to] > ranks[e.from] {
            parents[e.to].push(e.from);
            children[e.from].push(e.to);
        }
    }

    let mut pos = vec![0f64; n];
    for row in by_rank {
        let mut x = 0f64;
        for &v in row {
            let half = size[v] as f64 / 2.0;
            x += half;
            pos[v] = x;
            x += half + sep as f64;
        }
    }

    for it in 0..10 {
        if it % 2 == 0 {
            for row in by_rank.iter() {
                relax_rank(row, &parents, &mut pos, size, sep);
            }
        } else {
            for row in by_rank.iter().rev() {
                relax_rank(row, &children, &mut pos, size, sep);
            }
        }
    }

    let min_left = (0..n)
        .map(|v| pos[v] - size[v] as f64 / 2.0)
        .fold(f64::INFINITY, f64::min);
    let min_left = if min_left.is_finite() { min_left } else { 0.0 };
    (0..n)
        .map(|v| (pos[v] - min_left).round().max(0.0) as usize)
        .collect()
}

fn relax_rank(nodes: &[usize], neigh: &[Vec<usize>], pos: &mut [f64], size: &[usize], sep: usize) {
    let n = nodes.len();
    if n == 0 {
        return;
    }
    let desired: Vec<f64> = nodes
        .iter()
        .map(|&v| {
            if neigh[v].is_empty() {
                pos[v]
            } else {
                neigh[v].iter().map(|&u| pos[u]).sum::<f64>() / neigh[v].len() as f64
            }
        })
        .collect();

    let half = |i: usize| size[nodes[i]] as f64 / 2.0;
    let mut left = vec![0f64; n];
    let mut right = vec![0f64; n];
    for i in 0..n {
        left[i] = if i == 0 {
            desired[i]
        } else {
            desired[i].max(left[i - 1] + half(i - 1) + sep as f64 + half(i))
        };
    }
    for i in (0..n).rev() {
        right[i] = if i == n - 1 {
            desired[i]
        } else {
            desired[i].min(right[i + 1] - half(i + 1) - sep as f64 - half(i))
        };
    }
    for i in 0..n {
        pos[nodes[i]] = (left[i] + right[i]) / 2.0;
    }
    for i in 1..n {
        let min_p = pos[nodes[i - 1]] + half(i - 1) + sep as f64 + half(i);
        if pos[nodes[i]] < min_p {
            pos[nodes[i]] = min_p;
        }
    }
}

fn wrap_label(label: &str, width: usize, max_lines: usize) -> Vec<String> {
    let width = width.max(1);
    let char_w = |c: char| char_width(c).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in label.split_whitespace() {
        let ww = word.width();
        if ww > width {
            if !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for ch in word.chars() {
                let cw = char_w(ch);
                if chunk_w + cw > width && !chunk.is_empty() {
                    // Prefer breaking after the last identifier boundary so a long
                    // token is not sliced mid-segment; fall back to a per-char break.
                    let carry = match chunk.rfind(LABEL_BREAK_CHARS) {
                        Some(p) => chunk.split_off(p + 1),
                        None => String::new(),
                    };
                    lines.push(std::mem::take(&mut chunk));
                    chunk_w = carry.chars().map(char_w).sum();
                    chunk = carry;
                }
                chunk.push(ch);
                chunk_w += cw;
            }
            cur = chunk;
            cur_w = chunk_w;
        } else if cur.is_empty() {
            cur.push_str(word);
            cur_w = ww;
        } else if cur_w + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            cur_w += 1 + ww;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = ww;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let target = width.saturating_sub(1).max(1);
            let mut s = String::new();
            let mut sw = 0usize;
            for ch in last.chars() {
                let cw = char_w(ch);
                if sw + cw > target {
                    break;
                }
                s.push(ch);
                sw += cw;
            }
            s.push('…');
            *last = s;
        }
    }
    lines
}

fn fit_label(label: &str, inner: usize) -> String {
    if label.width() <= inner {
        return label.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for c in label.chars() {
        let cw = char_width(c);
        if used + cw + 1 > inner {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

fn draw_box(canvas: &mut Canvas, p: &Placed, lines: &[String], shape: Shape) {
    let (x, y, w, h) = (p.x, p.y, p.w, p.h);
    let right = x + w - 1;
    let bottom = y + h - 1;

    let (tl, tr, bl, br) = match shape {
        Shape::Round | Shape::Diamond => ('╭', '╮', '╰', '╯'),
        Shape::Rect => ('┌', '┐', '└', '┘'),
    };
    canvas.set(x, y, tl, Cls::Border);
    canvas.set(right, y, tr, Cls::Border);
    canvas.set(x, bottom, bl, Cls::Border);
    canvas.set(right, bottom, br, Cls::Border);

    for cx in (x + 1)..right {
        canvas.add_bits(cx, y, L | R);
        canvas.add_bits(cx, bottom, L | R);
    }
    for cy in (y + 1)..bottom {
        canvas.add_bits(x, cy, U | D);
        canvas.add_bits(right, cy, U | D);
    }

    for cy in y..=bottom {
        for cx in x..=right {
            let i = canvas.idx(cx, cy);
            canvas.occupied[i] = true;
        }
    }

    let inner = w.saturating_sub(2 * PAD + 2).max(1);
    for (li, line) in lines.iter().enumerate() {
        let row = y + 1 + li;
        let text = fit_label(line, inner);
        let tw = text.width();
        let text_x = x + 1 + PAD + inner.saturating_sub(tw) / 2;
        let mut cur = text_x;
        for c in text.chars() {
            let cw = char_width(c).max(1);
            canvas.set(cur, row, c, Cls::Text);
            // Wide glyphs (CJK, emoji) own a second column; mark it as a
            // continuation so the line builder doesn't emit a stray space.
            for k in 1..cw {
                canvas.set(cur + k, row, CONT, Cls::Text);
            }
            cur += cw;
        }
    }
}

fn route_forward(canvas: &mut Canvas, from: &Placed, to: &Placed, edge: &Edge, bus: usize) {
    let tx = to.cx;
    let bx = if from.cx.abs_diff(tx) <= 1 {
        tx
    } else {
        from.cx
    };
    let by = from.y + from.h - 1;
    let head_row = to.y - 1;

    canvas.junction(bx, by, D);
    canvas.seg_v(bx, by, bus);
    if bx == tx {
        canvas.seg_v(bx, bus, head_row);
    } else {
        canvas.seg_h(bus, bx, tx);
        canvas.seg_v(tx, bus, head_row);
    }

    if edge.head_to == Head::None {
        canvas.add_bits(tx, head_row, U);
    } else {
        canvas.set(tx, head_row, head_glyph(edge.head_to, '▼'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(bx, by, head_glyph(edge.head_from, '▲'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(canvas, label, head_row, tx + 1);
    }
}

fn head_glyph(head: Head, arrow: char) -> char {
    match head {
        Head::Circle => 'o',
        Head::Cross => '×',
        Head::DiamondFill => '◆',
        Head::DiamondOpen => '◇',
        Head::Triangle => match arrow {
            '▼' => '▽',
            '▲' => '△',
            '◄' => '◁',
            '▶' => '▷',
            other => other,
        },
        _ => arrow,
    }
}

fn route_self(canvas: &mut Canvas, p: &Placed, edge: &Edge) {
    let bottom = p.y + p.h - 1;
    let exit_x = p.cx + 1;
    let ret_x = p.x + p.w - 2;
    if ret_x <= exit_x || bottom + 2 >= canvas.h {
        return;
    }
    let (v, h, bl, br) = match edge.line {
        LineKind::Dotted => ('╎', '╌', '╰', '╯'),
        LineKind::Thick => ('┃', '━', '┗', '┛'),
        LineKind::Solid => ('│', '─', '╰', '╯'),
    };
    canvas.junction(exit_x, bottom, D);
    canvas.set(exit_x, bottom + 1, v, Cls::Edge);
    canvas.set(exit_x, bottom + 2, bl, Cls::Edge);
    for x in (exit_x + 1)..ret_x {
        canvas.set(x, bottom + 2, h, Cls::Edge);
    }
    canvas.set(ret_x, bottom + 2, br, Cls::Edge);
    canvas.set(ret_x, bottom + 1, head_glyph(edge.head_to, '▲'), Cls::Edge);
    if let Some(label) = &edge.label {
        place_label(canvas, label, bottom + 1, p.x + p.w + 1);
    }
}

fn route_back(canvas: &mut Canvas, from: &Placed, to: &Placed, edge: &Edge, lane_x: usize) {
    let sx = from.x + from.w - 1;
    let sy = from.cy;
    let tx = to.x + to.w - 1;
    let tyc = to.cy;

    canvas.junction(sx, sy, R);
    canvas.seg_h(sy, sx, lane_x);
    canvas.seg_v(lane_x, sy, tyc);
    canvas.seg_h(tyc, tx + 1, lane_x);

    if edge.head_to == Head::None {
        canvas.add_bits(tx + 1, tyc, R);
    } else {
        canvas.set(tx + 1, tyc, head_glyph(edge.head_to, '◄'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(sx, sy, head_glyph(edge.head_from, '◄'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(
            canvas,
            label,
            tyc.saturating_sub(1),
            lane_x.saturating_sub(label.width() + 1),
        );
    }
}

fn route_forward_lr(canvas: &mut Canvas, from: &Placed, to: &Placed, edge: &Edge, bus: usize) {
    let rx = from.x + from.w - 1;
    let ry = from.cy;
    let ly = to.cy;
    let head_col = to.x - 1;

    canvas.junction(rx, ry, R);
    canvas.seg_h(ry, rx, bus);
    if ry == ly {
        canvas.seg_h(ry, bus, head_col);
    } else {
        canvas.seg_v(bus, ry, ly);
        canvas.seg_h(ly, bus, head_col);
    }

    if edge.head_to == Head::None {
        canvas.add_bits(head_col, ly, R);
    } else {
        canvas.set(head_col, ly, head_glyph(edge.head_to, '▶'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(rx, ry, head_glyph(edge.head_from, '◄'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(canvas, label, ly.saturating_sub(1), bus + 1);
    }
}

fn route_back_lr(canvas: &mut Canvas, from: &Placed, to: &Placed, edge: &Edge, lane_y: usize) {
    let sx = from.cx;
    let sy = from.y + from.h - 1;
    let tx = to.cx;
    let ty = to.y + to.h - 1;

    canvas.junction(sx, sy, D);
    canvas.seg_v(sx, sy, lane_y);
    canvas.seg_h(lane_y, sx, tx);
    canvas.seg_v(tx, lane_y, ty + 1);

    if edge.head_to == Head::None {
        canvas.add_bits(tx, ty + 1, D);
    } else {
        canvas.set(tx, ty + 1, head_glyph(edge.head_to, '▲'), Cls::Edge);
    }
    if edge.head_from != Head::None {
        canvas.set(sx, sy, head_glyph(edge.head_from, '▲'), Cls::Edge);
    }

    if let Some(label) = &edge.label {
        place_label(canvas, label, lane_y.saturating_sub(1), (sx + tx) / 2);
    }
}

fn place_label(canvas: &mut Canvas, label: &str, row: usize, start_x: usize) {
    if row >= canvas.h {
        return;
    }
    let text = fit_label(label, MAX_LABEL);
    let mut x = start_x;
    for c in text.chars() {
        let cw = char_width(c).max(1);
        if x + cw > canvas.w {
            break;
        }
        let blocked = (0..cw).any(|k| {
            let i = canvas.idx(x + k, row);
            canvas.ch[i] != ' ' || canvas.mask[i] != 0 || canvas.occupied[i]
        });
        if blocked {
            break;
        }
        canvas.set(x, row, c, Cls::EdgeLabel);
        for k in 1..cw {
            canvas.set(x + k, row, CONT, Cls::EdgeLabel);
        }
        x += cw;
    }
}

fn compute_ranks(graph: &Graph) -> Vec<usize> {
    let n = graph.nodes.len();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for e in &graph.edges {
        if e.from != e.to {
            children[e.from].push(e.to);
            indeg[e.to] += 1;
        }
    }

    let mut color = vec![0u8; n];
    let mut dag: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut order: Vec<usize> = Vec::with_capacity(n);

    let roots: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    for start in roots.iter().copied().chain(0..n) {
        if color[start] == 0 {
            dfs_dag(start, &children, &mut color, &mut dag, &mut order);
        }
    }

    let mut rank = vec![0usize; n];
    for &u in order.iter().rev() {
        for &v in &dag[u] {
            rank[v] = rank[v].max(rank[u] + 1);
        }
    }
    rank
}

fn dfs_dag(
    start: usize,
    children: &[Vec<usize>],
    color: &mut [u8],
    dag: &mut [Vec<usize>],
    order: &mut Vec<usize>,
) {
    let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
    color[start] = 1;
    while let Some(frame) = stack.last_mut() {
        let u = frame.0;
        if frame.1 < children[u].len() {
            let v = children[u][frame.1];
            frame.1 += 1;
            if color[v] == 1 {
                continue;
            }
            dag[u].push(v);
            if color[v] == 0 {
                color[v] = 1;
                stack.push((v, 0));
            }
        } else {
            color[u] = 2;
            order.push(u);
            stack.pop();
        }
    }
}

const SEQ_GAP: usize = 5;
const SEQ_OPS: &[(&str, bool, SeqHead)] = &[
    ("-->>", true, SeqHead::Arrow),
    ("->>", false, SeqHead::Arrow),
    ("--x", true, SeqHead::Cross),
    ("-x", false, SeqHead::Cross),
    ("--)", true, SeqHead::Arrow),
    ("-)", false, SeqHead::Arrow),
    ("-->", true, SeqHead::Arrow),
    ("->", false, SeqHead::Arrow),
];

#[derive(Clone, Copy, PartialEq)]
enum SeqHead {
    Arrow,
    Cross,
}

enum NoteAnchor {
    Over(usize, usize),
    Left(usize),
    Right(usize),
}

enum SeqItem {
    Message {
        from: usize,
        to: usize,
        text: Option<String>,
        dashed: bool,
        head: SeqHead,
    },
    Note {
        anchor: NoteAnchor,
        text: String,
    },
    Divider {
        text: String,
    },
}
