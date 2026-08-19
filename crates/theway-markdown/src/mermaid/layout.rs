fn layout_flowchart(
    graph: &Graph,
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let extras: Vec<NodeExtra> = (0..graph.nodes.len()).map(|_| NodeExtra::Plain).collect();
    let mut canvas = layout_canvas(graph, &extras, max_width)?;
    match graph.dir {
        Dir::Up => canvas.flip_vertical(),
        Dir::Left => canvas.flip_horizontal(),
        _ => {}
    }
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
        fallback: false,
    })
}

enum NodeExtra {
    Plain,
    Frame(Canvas),
    Compartments(Vec<Vec<String>>),
}

fn layout_canvas(
    graph: &Graph,
    extras: &[NodeExtra],
    max_width: Option<usize>,
) -> Result<Canvas, Oversize> {
    let n = graph.nodes.len();
    if n == 0 {
        return Err(Oversize::Cells);
    }

    let ranks = compute_ranks(graph);
    let max_rank = *ranks.iter().max().unwrap_or(&0);

    let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    for (idx, &r) in ranks.iter().enumerate() {
        by_rank[r].push(idx);
    }
    order_ranks(&mut by_rank, &graph.edges, &ranks);

    let wrapped: Vec<Vec<String>> = graph
        .nodes
        .iter()
        .map(|node| wrap_label(&node.label, WRAP_WIDTH, MAX_LINES))
        .collect();
    let mut box_w: Vec<usize> = (0..n)
        .map(|i| match &extras[i] {
            NodeExtra::Frame(sub) => {
                let title_w = fit_label(&graph.nodes[i].label, WRAP_WIDTH).width();
                (sub.w + 2).max(title_w + 4)
            }
            NodeExtra::Compartments(sections) => {
                sections
                    .iter()
                    .flatten()
                    .map(|l| l.width())
                    .max()
                    .unwrap_or(1)
                    .max(1)
                    + 2 * PAD
                    + 2
            }
            NodeExtra::Plain => {
                wrapped[i]
                    .iter()
                    .map(|l| l.width())
                    .max()
                    .unwrap_or(1)
                    .max(1)
                    + 2 * PAD
                    + 2
            }
        })
        .collect();
    let box_h: Vec<usize> = (0..n)
        .map(|i| match &extras[i] {
            NodeExtra::Frame(sub) => sub.h + 2,
            NodeExtra::Compartments(sections) => {
                let filled = sections.iter().filter(|s| !s.is_empty()).count();
                sections.iter().map(|s| s.len()).sum::<usize>() + filled.saturating_sub(1) + 2
            }
            NodeExtra::Plain => wrapped[i].len() + 2,
        })
        .collect();

    let mut extra_h = vec![0usize; n];
    let mut self_label_w = vec![0usize; n];
    for e in &graph.edges {
        if e.from == e.to {
            extra_h[e.from] = 2;
            if let Some(l) = &e.label {
                self_label_w[e.from] = self_label_w[e.from].max(l.width().min(MAX_LABEL));
            }
        }
    }
    for i in 0..n {
        if extra_h[i] > 0 {
            box_w[i] = box_w[i].max(7);
        }
    }
    let lay_w: Vec<usize> = (0..n)
        .map(|i| {
            box_w[i]
                + if self_label_w[i] > 0 {
                    2 * (self_label_w[i] + 3)
                } else {
                    0
                }
        })
        .collect();
    let lay_h: Vec<usize> = (0..n).map(|i| box_h[i] + extra_h[i]).collect();
    let sizes = NodeSizes {
        box_w,
        box_h,
        lay_w,
        lay_h,
        extra_h,
        self_label_w,
    };

    let mut placed: Vec<Placed> = (0..n)
        .map(|_| Placed {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            cx: 0,
            cy: 0,
            rank: 0,
        })
        .collect();

    // BT/RL reuse the TD/LR layout, then flip the finished canvas (so text
    // stays readable) into the bottom-up / right-to-left orientation.
    let vertical = matches!(graph.dir, Dir::Down | Dir::Up);
    let plan = if vertical {
        place_td(&ranks, max_rank, &by_rank, &sizes, graph, &mut placed)
    } else {
        place_lr(&ranks, max_rank, &by_rank, &sizes, graph, &mut placed)
    };
    let (canvas_w, canvas_h) = plan.canvas;

    if let Some(mw) = max_width
        && canvas_w > mw
    {
        return Err(Oversize::Width);
    }
    if canvas_w.saturating_mul(canvas_h) > MAX_CANVAS_CELLS {
        return Err(Oversize::Cells);
    }

    let mut canvas = Canvas::new(canvas_w, canvas_h);
    for idx in 0..n {
        match &extras[idx] {
            NodeExtra::Frame(sub) => {
                draw_frame(&mut canvas, &placed[idx], &graph.nodes[idx].label, sub)
            }
            NodeExtra::Compartments(sections) => {
                draw_class_box(&mut canvas, &placed[idx], sections)
            }
            NodeExtra::Plain => draw_box(
                &mut canvas,
                &placed[idx],
                &wrapped[idx],
                graph.nodes[idx].shape,
            ),
        }
    }
    for (i, edge) in graph.edges.iter().enumerate() {
        canvas.cur_style = match edge.line {
            LineKind::Solid => STY_SOLID,
            LineKind::Dotted => STY_DOT,
            LineKind::Thick => STY_THICK,
        };
        if edge.from == edge.to {
            route_self(&mut canvas, &placed[edge.from], edge);
            continue;
        }
        let (from, to) = (&placed[edge.from], &placed[edge.to]);
        let adjacent = to.rank == from.rank + 1;
        let bus = plan.band_end[from.rank] + plan.edge_bus[i];
        let lane = plan.lane_base + plan.edge_lane[i];
        match (vertical, adjacent) {
            (true, true) => route_forward(&mut canvas, from, to, edge, bus),
            (true, false) => route_back(&mut canvas, from, to, edge, lane),
            (false, true) => route_forward_lr(&mut canvas, from, to, edge, bus),
            (false, false) => route_back_lr(&mut canvas, from, to, edge, lane),
        }
    }

    canvas.finalize_mask();
    Ok(canvas)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    Node(usize),
    Group(usize),
}

fn render_grouped(
    graph: &Graph,
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let mut proxy: HashMap<usize, usize> = HashMap::new();
    for (gi, g) in graph.groups.iter().enumerate() {
        if let Some(&ni) = graph.index.get(&g.id) {
            proxy.insert(ni, gi);
        }
    }

    let group_chain = |g: Option<usize>| -> Vec<usize> {
        let mut chain = Vec::new();
        let mut cur = g;
        while let Some(gi) = cur {
            chain.push(gi);
            cur = graph.groups[gi].parent;
        }
        chain.reverse();
        chain
    };
    let endpoint = |n: usize| -> (Item, Vec<usize>) {
        match proxy.get(&n) {
            Some(&gi) => (Item::Group(gi), group_chain(graph.groups[gi].parent)),
            None => (Item::Node(n), group_chain(graph.node_group[n])),
        }
    };

    let mut scope_edges: HashMap<Option<usize>, Vec<(Item, Item, usize)>> = HashMap::new();
    let mut referenced: Vec<bool> = vec![false; graph.groups.len()];
    for (ei, e) in graph.edges.iter().enumerate() {
        let (item_f, chain_f) = endpoint(e.from);
        let (item_t, chain_t) = endpoint(e.to);
        let k = chain_f
            .iter()
            .zip(&chain_t)
            .take_while(|(a, b)| a == b)
            .count();
        let scope = if k == 0 { None } else { Some(chain_f[k - 1]) };
        let f = if chain_f.len() > k {
            Item::Group(chain_f[k])
        } else {
            item_f
        };
        let t = if chain_t.len() > k {
            Item::Group(chain_t[k])
        } else {
            item_t
        };
        if let Item::Group(gi) = f {
            referenced[gi] = true;
        }
        if let Item::Group(gi) = t {
            referenced[gi] = true;
        }
        scope_edges.entry(scope).or_default().push((f, t, ei));
    }

    let mut direct_nodes: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (ni, g) in graph.node_group.iter().enumerate() {
        if !proxy.contains_key(&ni) {
            direct_nodes.entry(*g).or_default().push(ni);
        }
    }
    let mut keep = vec![false; graph.groups.len()];
    for gi in (0..graph.groups.len()).rev() {
        let has_nodes = direct_nodes.get(&Some(gi)).is_some_and(|v| !v.is_empty());
        let has_children =
            (0..graph.groups.len()).any(|c| graph.groups[c].parent == Some(gi) && keep[c]);
        keep[gi] = has_nodes || has_children || referenced[gi];
    }

    let mut canvas = build_scope(graph, None, &scope_edges, &direct_nodes, &keep, max_width)?;
    match graph.dir {
        Dir::Up => canvas.flip_vertical(),
        Dir::Left => canvas.flip_horizontal(),
        _ => {}
    }
    let (styled_lines, plain_lines) = canvas.to_lines(styles);
    Ok(MermaidArt {
        styled_lines,
        plain_lines,
        fallback: false,
    })
}

fn build_scope(
    graph: &Graph,
    scope: Option<usize>,
    scope_edges: &HashMap<Option<usize>, Vec<(Item, Item, usize)>>,
    direct_nodes: &HashMap<Option<usize>, Vec<usize>>,
    keep: &[bool],
    max_width: Option<usize>,
) -> Result<Canvas, Oversize> {
    let mut items: Vec<Item> = Vec::new();
    if let Some(nodes) = direct_nodes.get(&scope) {
        items.extend(nodes.iter().map(|&n| Item::Node(n)));
    }
    let child_groups: Vec<usize> = (0..graph.groups.len())
        .filter(|&gi| graph.groups[gi].parent == scope && keep[gi])
        .collect();
    items.extend(child_groups.iter().map(|&gi| Item::Group(gi)));

    if items.is_empty() {
        return Ok(Canvas::new(1, 1));
    }

    let mut index_of: HashMap<Item, usize> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut extras: Vec<NodeExtra> = Vec::new();
    for item in &items {
        index_of.insert(*item, nodes.len());
        match item {
            Item::Node(ni) => {
                nodes.push(Node {
                    label: graph.nodes[*ni].label.clone(),
                    shape: graph.nodes[*ni].shape,
                });
                extras.push(NodeExtra::Plain);
            }
            Item::Group(gi) => {
                let sub = build_scope(graph, Some(*gi), scope_edges, direct_nodes, keep, None)?;
                nodes.push(Node {
                    label: graph.groups[*gi].label.clone(),
                    shape: Shape::Rect,
                });
                extras.push(NodeExtra::Frame(sub));
            }
        }
    }

    let mut edges: Vec<Edge> = Vec::new();
    if let Some(list) = scope_edges.get(&scope) {
        for (f, t, ei) in list {
            let (Some(&fi), Some(&ti)) = (index_of.get(f), index_of.get(t)) else {
                continue;
            };
            let e = &graph.edges[*ei];
            edges.push(Edge {
                from: fi,
                to: ti,
                label: e.label.clone(),
                head_to: e.head_to,
                head_from: e.head_from,
                line: e.line,
            });
        }
    }

    let synth = Graph {
        nodes,
        edges,
        index: HashMap::new(),
        groups: Vec::new(),
        node_group: Vec::new(),
        cur_group: None,
        over_cap: false,
        dir: graph.dir,
    };
    layout_canvas(&synth, &extras, max_width)
}

fn draw_class_box(canvas: &mut Canvas, p: &Placed, sections: &[Vec<String>]) {
    draw_box(canvas, p, &[], Shape::Rect);
    let inner = p.w.saturating_sub(2 * PAD + 2).max(1);
    let mut row = p.y + 1;
    let mut first = true;
    for (si, section) in sections.iter().enumerate() {
        if section.is_empty() {
            continue;
        }
        if !first {
            canvas.set(p.x, row, '├', Cls::Border);
            for x in (p.x + 1)..(p.x + p.w - 1) {
                canvas.set(x, row, '─', Cls::Border);
            }
            canvas.set(p.x + p.w - 1, row, '┤', Cls::Border);
            row += 1;
        }
        first = false;
        for line in section {
            let text = fit_label(line, inner);
            let tx = if si == 0 {
                p.x + 1 + PAD + inner.saturating_sub(text.width()) / 2
            } else {
                p.x + 1 + PAD
            };
            draw_seq_text(canvas, &text, tx, row, Cls::Text);
            row += 1;
        }
    }
}

fn draw_frame(canvas: &mut Canvas, p: &Placed, title: &str, sub: &Canvas) {
    draw_box(canvas, p, &[], Shape::Rect);
    let t = fit_label(title, p.w.saturating_sub(4));
    draw_seq_text(canvas, &format!(" {t} "), p.x + 1, p.y, Cls::Text);
    let ox = p.x + 1 + (p.w - 2 - sub.w) / 2;
    let oy = p.y + 1 + (p.h - 2 - sub.h) / 2;
    canvas.blit(sub, ox, oy);
}

fn bus_spans_td(
    graph: &Graph,
    ranks: &[usize],
    centers: &[usize],
    r: usize,
    exact: bool,
) -> Vec<(usize, usize, usize, usize, usize)> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let jogs = if exact {
                centers[e.from] != centers[e.to]
            } else {
                centers[e.from].abs_diff(centers[e.to]) > 1
            };
            e.from != e.to && ranks[e.from] == r && ranks[e.to] == r + 1 && jogs
        })
        .map(|(i, e)| {
            let a = centers[e.from].min(centers[e.to]);
            let b = centers[e.from].max(centers[e.to]);
            (a, b, e.from, e.to, i)
        })
        .collect()
}

fn lane_spans(
    graph: &Graph,
    ranks: &[usize],
    placed: &[Placed],
    vertical: bool,
) -> Vec<(usize, usize, usize, usize, usize)> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to && ranks[e.to] != ranks[e.from] + 1)
        .map(|(i, e)| {
            let (pf, pt) = (&placed[e.from], &placed[e.to]);
            let (a, b) = if vertical {
                (pf.cy.min(pt.cy), pf.cy.max(pt.cy))
            } else {
                (pf.cx.min(pt.cx), pf.cx.max(pt.cx))
            };
            (a, b, e.from, e.to, i)
        })
        .collect()
}

fn place_td(
    ranks: &[usize],
    max_rank: usize,
    by_rank: &[Vec<usize>],
    sizes: &NodeSizes,
    graph: &Graph,
    placed: &mut [Placed],
) -> RoutePlan {
    let centers = assign_positions(by_rank, &sizes.lay_w, GAP_X, &graph.edges, ranks);

    let mut edge_bus = vec![0usize; graph.edges.len()];
    let mut bus_tracks = vec![0usize; max_rank + 1];
    for (r, tracks) in bus_tracks.iter_mut().enumerate().take(max_rank) {
        let spans = bus_spans_td(graph, ranks, &centers, r, false);
        if spans.is_empty() {
            continue;
        }
        let (assigned, count) = assign_tracks(&spans);
        for (idx, slot) in assigned {
            edge_bus[idx] = slot;
        }
        *tracks = count;
    }

    let rank_h: Vec<usize> = by_rank
        .iter()
        .map(|row| {
            row.iter()
                .map(|&i| sizes.box_h[i] + sizes.extra_h[i])
                .max()
                .unwrap_or(3)
        })
        .collect();
    let mut rank_y = vec![0usize; max_rank + 1];
    for r in 1..=max_rank {
        let gap = GAP_Y.max(bus_tracks[r - 1] + 1);
        rank_y[r] = rank_y[r - 1] + rank_h[r - 1] + gap;
    }
    let canvas_h = rank_y[max_rank] + rank_h[max_rank];
    let band_end: Vec<usize> = (0..=max_rank).map(|r| rank_y[r] + rank_h[r]).collect();

    let mut diagram_w = 1;
    for (r, row) in by_rank.iter().enumerate() {
        for &idx in row {
            let w = sizes.box_w[idx];
            let h = sizes.box_h[idx];
            let cx = centers[idx];
            let x = cx.saturating_sub(w / 2);
            let y = rank_y[r] + (rank_h[r] - h - sizes.extra_h[idx]) / 2;
            placed[idx] = Placed {
                x,
                y,
                w,
                h,
                cx,
                cy: y + h / 2,
                rank: r,
            };
            diagram_w = diagram_w.max(x + w);
            if sizes.extra_h[idx] > 0 && sizes.self_label_w[idx] > 0 {
                diagram_w = diagram_w.max(x + w + 2 + sizes.self_label_w[idx]);
            }
        }
    }

    let mut content_w = diagram_w;
    for e in &graph.edges {
        if e.from == e.to {
            continue;
        }
        if let Some(label) = &e.label {
            let lw = label.width().min(MAX_LABEL);
            if ranks[e.to] == ranks[e.from] + 1 {
                content_w = content_w.max(placed[e.to].cx + 2 + lw);
            } else {
                content_w = content_w.max(diagram_w + lw + 1);
            }
        }
    }

    let mut edge_lane = vec![0usize; graph.edges.len()];
    let lanes = lane_spans(graph, ranks, placed, true);
    let (canvas_w, lane_base) = if lanes.is_empty() {
        (content_w, 0)
    } else {
        let (assigned, count) = assign_tracks(&lanes);
        for (idx, slot) in assigned {
            edge_lane[idx] = slot;
        }
        (content_w + 1 + count, content_w + 1)
    };

    RoutePlan {
        canvas: (canvas_w, canvas_h),
        band_end,
        edge_bus,
        lane_base,
        edge_lane,
    }
}

fn place_lr(
    ranks: &[usize],
    max_rank: usize,
    by_rank: &[Vec<usize>],
    sizes: &NodeSizes,
    graph: &Graph,
    placed: &mut [Placed],
) -> RoutePlan {
    let col_w: Vec<usize> = by_rank
        .iter()
        .map(|row| row.iter().map(|&i| sizes.box_w[i]).max().unwrap_or(0))
        .collect();

    let max_label = graph
        .edges
        .iter()
        .filter(|e| e.from == e.to || ranks[e.to] == ranks[e.from] + 1)
        .filter_map(|e| e.label.as_ref().map(|l| l.width().min(MAX_LABEL)))
        .max()
        .unwrap_or(0);
    let base_gap = (GAP_X + 1).max(max_label + 3);

    let centers = assign_positions(by_rank, &sizes.lay_h, 1, &graph.edges, ranks);

    let mut edge_bus = vec![0usize; graph.edges.len()];
    let mut bus_tracks = vec![0usize; max_rank + 1];
    for (r, tracks) in bus_tracks.iter_mut().enumerate().take(max_rank) {
        let spans = bus_spans_td(graph, ranks, &centers, r, true);
        if spans.is_empty() {
            continue;
        }
        let (assigned, count) = assign_tracks(&spans);
        for (idx, slot) in assigned {
            edge_bus[idx] = slot;
        }
        *tracks = count;
    }

    let mut rank_x = vec![0usize; max_rank + 1];
    for r in 1..=max_rank {
        let gap = base_gap.max(bus_tracks[r - 1] + 1);
        rank_x[r] = rank_x[r - 1] + col_w[r - 1] + gap;
    }
    let canvas_w = rank_x[max_rank]
        + col_w[max_rank]
        + by_rank[max_rank]
            .iter()
            .filter(|&&i| sizes.extra_h[i] > 0 && sizes.self_label_w[i] > 0)
            .map(|&i| 2 + sizes.self_label_w[i])
            .max()
            .unwrap_or(0);
    let band_end: Vec<usize> = (0..=max_rank).map(|r| rank_x[r] + col_w[r]).collect();

    let mut diagram_h = 1;
    for (r, row) in by_rank.iter().enumerate() {
        let x = rank_x[r];
        for &idx in row {
            let w = sizes.box_w[idx];
            let h = sizes.box_h[idx];
            let cy = centers[idx];
            let y = cy.saturating_sub((h + sizes.extra_h[idx]) / 2);
            placed[idx] = Placed {
                x,
                y,
                w,
                h,
                cx: x + w / 2,
                cy: y + h / 2,
                rank: r,
            };
            diagram_h = diagram_h.max(y + h + sizes.extra_h[idx]);
        }
    }

    let mut edge_lane = vec![0usize; graph.edges.len()];
    let lanes = lane_spans(graph, ranks, placed, false);
    let (canvas_h, lane_base) = if lanes.is_empty() {
        (diagram_h, 0)
    } else {
        let (assigned, count) = assign_tracks(&lanes);
        for (idx, slot) in assigned {
            edge_lane[idx] = slot;
        }
        (diagram_h + 1 + count, diagram_h + 1)
    };

    RoutePlan {
        canvas: (canvas_w, canvas_h),
        band_end,
        edge_bus,
        lane_base,
        edge_lane,
    }
}

struct RoutePlan {
    canvas: (usize, usize),
    band_end: Vec<usize>,
    edge_bus: Vec<usize>,
    lane_base: usize,
    edge_lane: Vec<usize>,
}
