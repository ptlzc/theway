fn parse_state(src: &str) -> Option<Graph> {
    let mut statements: Vec<String> = Vec::new();
    for raw_line in src.lines() {
        split_statements(raw_line, &mut statements);
    }
    let header = statements.first()?;
    if !header
        .split_whitespace()
        .next()?
        .to_ascii_lowercase()
        .starts_with("statediagram")
    {
        return None;
    }

    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        index: HashMap::new(),
        groups: Vec::new(),
        node_group: Vec::new(),
        cur_group: None,
        over_cap: false,
        dir: Dir::Down,
    };

    let mut in_note = false;
    for st in &statements[1..] {
        if in_note {
            if st.eq_ignore_ascii_case("end note") {
                in_note = false;
            }
            continue;
        }
        let first = st.split_whitespace().next().unwrap_or("");
        match first.to_ascii_lowercase().as_str() {
            "direction" => {
                graph.dir = match st
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .as_str()
                {
                    "LR" => Dir::Right,
                    "RL" => Dir::Left,
                    "BT" => Dir::Up,
                    _ => Dir::Down,
                };
            }
            "note" => {
                if !st.contains(':') {
                    in_note = true;
                }
            }
            "state" => parse_state_decl(st, &mut graph)?,
            "classdef" | "class" | "hide" | "scale" | "}" | "--" => {}
            _ => {
                if st.contains("-->") {
                    parse_transition(st, &mut graph)?;
                } else {
                    parse_state_desc(st, &mut graph)?;
                }
            }
        }
        if graph.over_cap {
            return None;
        }
    }

    if graph.nodes.is_empty() {
        return None;
    }
    Some(graph)
}

fn parse_state_decl(st: &str, graph: &mut Graph) -> Option<()> {
    let rest = st["state".len()..].trim().trim_end_matches('{').trim();
    if rest.is_empty() {
        return Some(());
    }
    if let Some(q) = rest.strip_prefix('"') {
        let (label, after) = q.split_once('"')?;
        let id = after
            .trim()
            .strip_prefix("as")
            .map(str::trim)
            .unwrap_or(label);
        graph.node_label(id, &decode_html_entities(label))?;
        return Some(());
    }
    let mut shape = Shape::Round;
    let mut id = rest;
    let mut stereotyped = false;
    if let Some(pos) = rest.find("<<") {
        let stereo = rest[pos + 2..].trim_end_matches(">>").trim();
        if stereo == "choice" {
            shape = Shape::Diamond;
        }
        id = rest[..pos].trim();
        stereotyped = true;
    }
    if id.is_empty() || id.contains(char::is_whitespace) {
        return None;
    }
    let label = if stereotyped { Some(id) } else { None };
    graph.node_index(id, label, shape)?;
    Some(())
}

fn parse_transition(st: &str, graph: &mut Graph) -> Option<()> {
    let mut rest = st;
    let mut prev: Option<usize> = None;
    while let Some((lhs, rhs)) = rest.split_once("-->") {
        let from_id = lhs.trim_end().trim_end_matches('-').trim();
        let from = match prev {
            Some(p) => {
                if !from_id.is_empty() {
                    return None;
                }
                p
            }
            None => {
                if from_id.is_empty() {
                    return None;
                }
                state_endpoint(graph, from_id, true)?
            }
        };
        let (to_part, tail) = match rhs.split_once("-->") {
            Some((t, _)) => (t, &rhs[t.len()..]),
            None => (rhs, ""),
        };
        let (to_part, label) = match to_part.split_once(':') {
            Some((t, l)) => (t, non_empty(decode_html_entities(l.trim()))),
            None => (to_part, None),
        };
        let to_id = to_part
            .trim_start()
            .trim_start_matches('>')
            .trim_end()
            .trim_end_matches('-')
            .trim();
        if to_id.is_empty() {
            return None;
        }
        let to = state_endpoint(graph, to_id, false)?;
        if graph.edges.len() >= MAX_EDGES {
            graph.over_cap = true;
            return Some(());
        }
        graph.edges.push(Edge {
            from,
            to,
            label,
            head_to: Head::Arrow,
            head_from: Head::None,
            line: LineKind::Solid,
        });
        prev = Some(to);
        rest = tail;
    }
    Some(())
}

fn state_endpoint(graph: &mut Graph, id: &str, is_source: bool) -> Option<usize> {
    if id == "[*]" {
        let key = if is_source { "[*]start" } else { "[*]end" };
        return graph.node_index(key, Some("●"), Shape::Round);
    }
    graph.node_index(id, None, Shape::Round)
}

fn parse_state_desc(st: &str, graph: &mut Graph) -> Option<()> {
    if let Some((id, desc)) = st.split_once(':') {
        let id = id.trim();
        let desc = desc.trim();
        if id.is_empty() || id.contains(char::is_whitespace) || desc.is_empty() {
            return None;
        }
        graph.node_label(id, &decode_html_entities(desc))?;
    } else if !st.contains(char::is_whitespace) {
        graph.node_index(st, None, Shape::Round)?;
    } else {
        return None;
    }
    Some(())
}

const MAX_MEMBERS: usize = 8;
const CLASS_OPS: &[(&str, Head, Head, LineKind)] = &[
    ("<|--", Head::Triangle, Head::None, LineKind::Solid),
    ("--|>", Head::None, Head::Triangle, LineKind::Solid),
    ("<|..", Head::Triangle, Head::None, LineKind::Dotted),
    ("..|>", Head::None, Head::Triangle, LineKind::Dotted),
    ("*--", Head::DiamondFill, Head::None, LineKind::Solid),
    ("--*", Head::None, Head::DiamondFill, LineKind::Solid),
    ("o--", Head::DiamondOpen, Head::None, LineKind::Solid),
    ("--o", Head::None, Head::DiamondOpen, LineKind::Solid),
    ("<--", Head::Arrow, Head::None, LineKind::Solid),
    ("-->", Head::None, Head::Arrow, LineKind::Solid),
    ("<..", Head::Arrow, Head::None, LineKind::Dotted),
    ("..>", Head::None, Head::Arrow, LineKind::Dotted),
    ("--", Head::None, Head::None, LineKind::Solid),
    ("..", Head::None, Head::None, LineKind::Dotted),
];

#[derive(Default, Clone)]
struct ClassInfo {
    annotation: Option<String>,
    attrs: Vec<String>,
    methods: Vec<String>,
}

fn parse_class(src: &str) -> Option<(Graph, Vec<ClassInfo>)> {
    let mut statements: Vec<String> = Vec::new();
    for raw_line in src.lines() {
        split_statements(raw_line, &mut statements);
    }
    let header = statements.first()?;
    if !header
        .split_whitespace()
        .next()?
        .to_ascii_lowercase()
        .starts_with("classdiagram")
    {
        return None;
    }

    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        index: HashMap::new(),
        groups: Vec::new(),
        node_group: Vec::new(),
        cur_group: None,
        over_cap: false,
        dir: Dir::Down,
    };
    let mut infos: Vec<ClassInfo> = Vec::new();
    let mut cur_class: Option<usize> = None;

    for st in &statements[1..] {
        if let Some(ci) = cur_class {
            if st == "}" {
                cur_class = None;
            } else {
                push_member(&mut infos[ci], st);
            }
            continue;
        }
        let first = st.split_whitespace().next().unwrap_or("");
        match first.to_ascii_lowercase().as_str() {
            "direction" => {
                graph.dir = match st
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_ascii_uppercase()
                    .as_str()
                {
                    "LR" => Dir::Right,
                    "RL" => Dir::Left,
                    "BT" => Dir::Up,
                    _ => Dir::Down,
                };
                continue;
            }
            "note" | "callback" | "click" | "link" | "style" | "cssclass" | "classdef"
            | "namespace" | "}" => continue,
            "class" => {
                let rest = st["class".len()..].trim();
                let (name, open) = match rest.strip_suffix('{') {
                    Some(n) => (n.trim(), true),
                    None => (rest, false),
                };
                if name.is_empty() || name.contains(char::is_whitespace) {
                    return None;
                }
                let idx = graph.node_index(name, None, Shape::Rect)?;
                sync_infos(&graph, &mut infos);
                if open {
                    cur_class = Some(idx);
                }
                continue;
            }
            _ => {}
        }
        if let Some(ann) = st.strip_prefix("<<") {
            let (ann, rest) = ann.split_once(">>")?;
            let name = rest.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            let idx = graph.node_index(name, None, Shape::Rect)?;
            sync_infos(&graph, &mut infos);
            infos[idx].annotation = Some(ann.trim().to_string());
            continue;
        }
        if let Some((from, to, head_from, head_to, line, label)) = parse_class_relation(st) {
            let f = graph.node_index(&from, None, Shape::Rect)?;
            sync_infos(&graph, &mut infos);
            let t = graph.node_index(&to, None, Shape::Rect)?;
            sync_infos(&graph, &mut infos);
            if graph.edges.len() >= MAX_EDGES {
                return None;
            }
            graph.edges.push(Edge {
                from: f,
                to: t,
                label,
                head_to,
                head_from,
                line,
            });
            continue;
        }
        if let Some((id, member)) = st.split_once(':') {
            let id = id.trim();
            let member = member.trim();
            if id.is_empty() || id.contains(char::is_whitespace) || member.is_empty() {
                return None;
            }
            let idx = graph.node_index(id, None, Shape::Rect)?;
            sync_infos(&graph, &mut infos);
            push_member(&mut infos[idx], member);
            continue;
        }
        return None;
    }

    if graph.nodes.is_empty() {
        return None;
    }
    sync_infos(&graph, &mut infos);
    Some((graph, infos))
}

fn sync_infos(graph: &Graph, infos: &mut Vec<ClassInfo>) {
    while infos.len() < graph.nodes.len() {
        infos.push(ClassInfo::default());
    }
}

fn push_member(info: &mut ClassInfo, raw: &str) {
    if let Some(ann) = raw.strip_prefix("<<") {
        if let Some((ann, _)) = ann.split_once(">>") {
            info.annotation = Some(ann.trim().to_string());
        }
        return;
    }
    let member = decode_html_entities(&display_generics(raw.trim()));
    let list = if member.contains('(') {
        &mut info.methods
    } else {
        &mut info.attrs
    };
    if list.len() < MAX_MEMBERS {
        list.push(member);
    } else if list.len() == MAX_MEMBERS {
        list.push("…".to_string());
    }
}

fn parse_class_relation(
    st: &str,
) -> Option<(String, String, Head, Head, LineKind, Option<String>)> {
    let chars: Vec<char> = st.chars().collect();
    let mut found: Option<(usize, &str, Head, Head, LineKind)> = None;
    'outer: for pos in 0..chars.len() {
        for &(op, hf, ht, line) in CLASS_OPS {
            if st[char_byte(st, pos)..].starts_with(op) {
                if op.starts_with('o') && pos > 0 && is_id_char(chars[pos - 1]) {
                    continue;
                }
                if op.ends_with('o')
                    && chars
                        .get(pos + op.chars().count())
                        .is_some_and(|&c| is_id_char(c))
                {
                    continue;
                }
                found = Some((pos, op, hf, ht, line));
                break 'outer;
            }
        }
    }
    let (pos, op, head_from, head_to, line) = found?;
    let lhs = st[..char_byte(st, pos)].trim();
    let rhs = st[char_byte(st, pos) + op.len()..].trim();

    let (lhs, card_from) = strip_cardinality_suffix(lhs);
    let (rhs, card_to) = strip_cardinality_prefix(rhs);
    let (to_id, rel_label) = match rhs.split_once(':') {
        Some((t, l)) => (t.trim(), non_empty(decode_html_entities(l.trim()))),
        None => (rhs.trim(), None),
    };
    if lhs.is_empty()
        || to_id.is_empty()
        || lhs.contains(char::is_whitespace)
        || to_id.contains(char::is_whitespace)
    {
        return None;
    }
    let label = non_empty(
        [card_from, rel_label.unwrap_or_default(), card_to]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
    );
    Some((
        lhs.to_string(),
        to_id.to_string(),
        head_from,
        head_to,
        line,
        label,
    ))
}

fn char_byte(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn strip_cardinality_suffix(s: &str) -> (&str, String) {
    let t = s.trim_end();
    if let Some(rest) = t.strip_suffix('"')
        && let Some(q) = rest.rfind('"')
    {
        return (rest[..q].trim_end(), rest[q + 1..].to_string());
    }
    (t, String::new())
}

fn strip_cardinality_prefix(s: &str) -> (&str, String) {
    let t = s.trim_start();
    if let Some(rest) = t.strip_prefix('"')
        && let Some(q) = rest.find('"')
    {
        return (rest[q + 1..].trim_start(), rest[..q].to_string());
    }
    (t, String::new())
}

fn display_generics(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut open = false;
    for c in s.chars() {
        if c == '~' {
            out.push(if open { '>' } else { '<' });
            open = !open;
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_er(src: &str) -> Option<(Graph, Vec<ClassInfo>)> {
    let mut statements: Vec<String> = Vec::new();
    for raw_line in src.lines() {
        split_statements(raw_line, &mut statements);
    }
    let header = statements.first()?;
    if !header
        .split_whitespace()
        .next()?
        .eq_ignore_ascii_case("erdiagram")
    {
        return None;
    }

    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        index: HashMap::new(),
        groups: Vec::new(),
        node_group: Vec::new(),
        cur_group: None,
        over_cap: false,
        dir: Dir::Down,
    };
    let mut infos: Vec<ClassInfo> = Vec::new();
    let mut cur_entity: Option<usize> = None;

    for st in &statements[1..] {
        if let Some(ei) = cur_entity {
            if st == "}" {
                cur_entity = None;
            } else {
                push_er_attribute(&mut infos[ei], st);
            }
            continue;
        }
        if let Some((rel, label_part)) = split_er_relationship(st) {
            let tokens: Vec<&str> = rel.split_whitespace().collect();
            let [lhs, op, rhs] = tokens.as_slice() else {
                return None;
            };
            let (card_l, card_r, line) = parse_er_op(op)?;
            let f = er_entity(&mut graph, &mut infos, lhs)?;
            let t = er_entity(&mut graph, &mut infos, rhs)?;
            if graph.edges.len() >= MAX_EDGES {
                return None;
            }
            let rel_label = label_part.map(clean_label).unwrap_or_default();
            let label = non_empty(
                [card_l.to_string(), rel_label, card_r.to_string()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            graph.edges.push(Edge {
                from: f,
                to: t,
                label,
                head_to: Head::None,
                head_from: Head::None,
                line,
            });
            continue;
        }
        let (decl, open) = match st.strip_suffix('{') {
            Some(d) => (d.trim(), true),
            None => (st.as_str(), false),
        };
        if decl.is_empty() || decl.split_whitespace().count() != 1 {
            return None;
        }
        let idx = er_entity(&mut graph, &mut infos, decl)?;
        if open {
            cur_entity = Some(idx);
        }
    }

    if graph.nodes.is_empty() {
        return None;
    }
    sync_infos(&graph, &mut infos);
    Some((graph, infos))
}

fn er_entity(graph: &mut Graph, infos: &mut Vec<ClassInfo>, token: &str) -> Option<usize> {
    let idx = if let Some(open) = token.find('[') {
        let id = &token[..open];
        let label = clean_label(token[open + 1..].trim_end_matches(']'));
        if id.is_empty() || label.is_empty() {
            return None;
        }
        graph.node_label(id, &label)?
    } else {
        graph.node_index(token, None, Shape::Rect)?
    };
    sync_infos(graph, infos);
    Some(idx)
}

fn split_er_relationship(st: &str) -> Option<(&str, Option<&str>)> {
    let (rel, label) = match st.split_once(':') {
        Some((r, l)) => (r, Some(l.trim())),
        None => (st, None),
    };
    let has_op = rel.split_whitespace().any(|t| parse_er_op(t).is_some());
    if has_op { Some((rel, label)) } else { None }
}

fn parse_er_op(tok: &str) -> Option<(&'static str, &'static str, LineKind)> {
    if !tok.is_ascii() || tok.len() != 6 {
        return None;
    }
    let line = match &tok[2..4] {
        "--" => LineKind::Solid,
        ".." => LineKind::Dotted,
        _ => return None,
    };
    Some((er_card(&tok[..2])?, er_card(&tok[4..6])?, line))
}

fn er_card(tok: &str) -> Option<&'static str> {
    match tok {
        "|o" | "o|" => Some("0..1"),
        "||" => Some("1"),
        "}o" | "o{" => Some("0..*"),
        "}|" | "|{" => Some("1..*"),
        _ => None,
    }
}

fn push_er_attribute(info: &mut ClassInfo, raw: &str) {
    let mut parts: Vec<&str> = Vec::new();
    for tok in raw.split_whitespace() {
        if tok.starts_with('"') {
            break;
        }
        parts.push(tok);
    }
    if parts.is_empty() {
        return;
    }
    let line = decode_html_entities(&parts.join(" "));
    if info.attrs.len() < MAX_MEMBERS {
        info.attrs.push(line);
    } else if info.attrs.len() == MAX_MEMBERS {
        info.attrs.push("…".to_string());
    }
}

fn render_class(
    graph: &Graph,
    infos: &[ClassInfo],
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    let extras: Vec<NodeExtra> = graph
        .nodes
        .iter()
        .zip(infos)
        .map(|(node, info)| {
            let mut title = Vec::new();
            if let Some(a) = &info.annotation {
                title.push(format!("«{a}»"));
            }
            title.push(display_generics(&node.label));
            NodeExtra::Compartments(vec![title, info.attrs.clone(), info.methods.clone()])
        })
        .collect();
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

const U: u8 = 1;
const D: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;

#[derive(Clone, Copy, PartialEq)]
enum Cls {
    Empty,
    Border,
    Text,
    Edge,
    EdgeLabel,
}

const STY_DOT: u8 = 1;
const STY_THICK: u8 = 2;
const STY_SOLID: u8 = 4;
