fn parse_graph(src: &str) -> Option<Graph> {
    let mut statements: Vec<String> = Vec::new();
    for raw_line in src.lines() {
        split_statements(raw_line, &mut statements);
    }

    let header = statements.first()?;
    let mut header_tokens = header.split_whitespace();
    let kind = header_tokens.next()?.to_ascii_lowercase();
    if kind != "graph" && kind != "flowchart" {
        return None;
    }
    let dir = match header_tokens
        .next()
        .unwrap_or("TB")
        .to_ascii_uppercase()
        .as_str()
    {
        "LR" => Dir::Right,
        "RL" => Dir::Left,
        "BT" => Dir::Up,
        _ => Dir::Down,
    };

    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        index: HashMap::new(),
        groups: Vec::new(),
        node_group: Vec::new(),
        cur_group: None,
        over_cap: false,
        dir,
    };

    let mut stack: Vec<usize> = Vec::new();
    for st in &statements[1..] {
        let first_word = st.split_whitespace().next().unwrap_or("");
        match first_word.to_ascii_lowercase().as_str() {
            "subgraph" => {
                if graph.groups.len() >= MAX_GROUPS || stack.len() >= MAX_GROUP_DEPTH {
                    return None;
                }
                let (id, label) = parse_subgraph_decl(st["subgraph".len()..].trim());
                graph.groups.push(Group {
                    id,
                    label,
                    parent: stack.last().copied(),
                });
                stack.push(graph.groups.len() - 1);
                graph.cur_group = stack.last().copied();
                continue;
            }
            "end" => {
                stack.pop();
                graph.cur_group = stack.last().copied();
                continue;
            }
            "classdef" | "class" | "style" | "linkstyle" | "click" | "direction" => continue,
            _ => {}
        }
        parse_statement(st, &mut graph);
        if graph.over_cap {
            return None;
        }
    }

    if graph.nodes.is_empty() {
        return None;
    }
    Some(graph)
}

fn parse_subgraph_decl(rest: &str) -> (String, String) {
    if let Some(q) = rest.strip_prefix('"')
        && let Some((label, _)) = q.split_once('"')
    {
        return (label.to_string(), decode_html_entities(label));
    }
    if let Some(open) = rest.find('[') {
        let id = rest[..open].trim();
        let label = rest[open + 1..].trim_end_matches(']').trim();
        let label = clean_label(label);
        if !id.is_empty() && !label.is_empty() {
            return (id.to_string(), label);
        }
    }
    (rest.to_string(), rest.to_string())
}

fn split_statements(line: &str, out: &mut Vec<String>) {
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                in_quotes = false;
            }
            cur.push(c);
        } else {
            match c {
                '"' => {
                    in_quotes = true;
                    cur.push(c);
                }
                '%' if chars.peek() == Some(&'%') => break,
                ';' => flush_statement(&mut cur, out),
                _ => cur.push(c),
            }
        }
    }
    flush_statement(&mut cur, out);
}

fn flush_statement(cur: &mut String, out: &mut Vec<String>) {
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    cur.clear();
}

fn parse_statement(st: &str, graph: &mut Graph) {
    let chars: Vec<char> = st.chars().collect();
    let mut i = 0;

    let Some((mut prev, ni)) = parse_node_group(&chars, i, graph) else {
        return;
    };
    i = ni;

    loop {
        i = skip_spaces(&chars, i);
        if i >= chars.len() {
            break;
        }
        let Some((left, right, line, label, ni)) = parse_link(&chars, i) else {
            break;
        };
        i = skip_spaces(&chars, ni);
        let Some((next, ni)) = parse_node_group(&chars, i, graph) else {
            break;
        };
        i = ni;
        for &f in &prev {
            for &t in &next {
                if graph.edges.len() >= MAX_EDGES {
                    graph.over_cap = true;
                    return;
                }
                let (from, to, head_to, head_from) = if left == Head::Arrow && right != Head::Arrow
                {
                    (t, f, Head::Arrow, right)
                } else {
                    (f, t, right, left)
                };
                graph.edges.push(Edge {
                    from,
                    to,
                    label: label.clone(),
                    head_to,
                    head_from,
                    line,
                });
            }
        }
        prev = next;
    }
}

fn parse_node_group(
    chars: &[char],
    start: usize,
    graph: &mut Graph,
) -> Option<(Vec<usize>, usize)> {
    let (first, mut i) = parse_node(chars, start, graph)?;
    let mut group = vec![first];
    loop {
        let j = skip_spaces(chars, i);
        if chars.get(j) != Some(&'&') {
            break;
        }
        let (next, k) = parse_node(chars, j + 1, graph)?;
        group.push(next);
        i = k;
    }
    Some((group, i))
}

fn skip_spaces(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    i
}

fn is_id_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn parse_node(chars: &[char], start: usize, graph: &mut Graph) -> Option<(usize, usize)> {
    let mut i = skip_spaces(chars, start);
    let id_start = i;
    while i < chars.len() && is_id_char(chars[i]) {
        i += 1;
    }
    if i == id_start {
        return None;
    }
    let id: String = chars[id_start..i].iter().collect();

    let (shape, label, after) = match chars.get(i) {
        Some('[') => {
            if chars.get(i + 1) == Some(&'[') {
                read_shape(chars, i + 2, "]]", Shape::Rect)
            } else if chars.get(i + 1) == Some(&'(') {
                read_shape(chars, i + 2, ")]", Shape::Round)
            } else {
                read_shape(chars, i + 1, "]", Shape::Rect)
            }
        }
        Some('(') => {
            if chars.get(i + 1) == Some(&'(') {
                read_shape(chars, i + 2, "))", Shape::Round)
            } else if chars.get(i + 1) == Some(&'[') {
                read_shape(chars, i + 2, "])", Shape::Round)
            } else {
                read_shape(chars, i + 1, ")", Shape::Round)
            }
        }
        Some('{') => {
            if chars.get(i + 1) == Some(&'{') {
                read_shape(chars, i + 2, "}}", Shape::Diamond)
            } else {
                read_shape(chars, i + 1, "}", Shape::Diamond)
            }
        }
        Some('>') => read_shape(chars, i + 1, "]", Shape::Rect),
        _ => (None, None, i),
    };

    let shape = shape.unwrap_or(Shape::Rect);
    let label = label.as_deref();
    let idx = graph.node_index(&id, label, shape)?;
    Some((idx, after))
}

fn read_shape(
    chars: &[char],
    start: usize,
    closer: &str,
    shape: Shape,
) -> (Option<Shape>, Option<String>, usize) {
    let closer: Vec<char> = closer.chars().collect();
    let mut i = start;
    let mut text = String::new();
    let quoted = {
        let mut j = start;
        while matches!(chars.get(j), Some(' ') | Some('\t')) {
            j += 1;
        }
        chars.get(j) == Some(&'"')
    };
    let mut in_quotes = false;
    while i < chars.len() {
        let c = chars[i];
        if quoted && c == '"' {
            in_quotes = !in_quotes;
            text.push(c);
            i += 1;
            continue;
        }
        if !in_quotes && chars[i..].starts_with(closer.as_slice()) {
            let label = clean_label(&text);
            return (Some(shape), Some(label), i + closer.len());
        }
        text.push(c);
        i += 1;
    }
    (Some(shape), Some(clean_label(&text)), chars.len())
}

fn clean_label(raw: &str) -> String {
    let stripped = strip_html_tags(raw.trim());
    let trimmed = stripped.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .trim();
    let text = if let Some(md) = unquoted.strip_prefix('`').and_then(|t| t.strip_suffix('`')) {
        strip_markdown(md.trim())
    } else {
        unquoted.to_string()
    };
    // Decode after tag-stripping so `<b>` is removed as markup while `&lt;b&gt;`
    // survives as a literal `<b>`; one decode at the single return covers both paths.
    decode_html_entities(&text)
}

const ENTITY_LOOKAHEAD: usize = 10;

// Label text decodes HTML entities once: via clean_label for bracketed labels, or explicitly at each direct-push sink.
fn decode_html_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Scan window (includes the terminating `;`) so a stray `&` or over-long run stays literal.
        let hi = (i + 1 + ENTITY_LOOKAHEAD).min(chars.len());
        let semi = (i + 1..hi).find(|&j| chars[j] == ';');
        let decoded = semi.and_then(|j| {
            let body: String = chars[i + 1..j].iter().collect();
            decode_entity_body(&body).map(|c| (c, j))
        });
        match decoded {
            // Resume past the `;`; the single pass never re-scans emitted text, so
            // `&amp;lt;` decodes to the literal `&lt;` rather than to `<`.
            Some((c, j)) => {
                out.push(c);
                i = j + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

fn decode_entity_body(body: &str) -> Option<char> {
    match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let num = body.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            // Reject control chars: NUL collides with the CONT sentinel and ESC would inject ANSI into scrollback.
            char::from_u32(code).filter(|c| !c.is_control())
        }
    }
}

fn strip_markdown(s: &str) -> String {
    let no_code: String = s.chars().filter(|&c| c != '`').collect();
    let no_strong = no_code.replace("**", "").replace("__", "");
    let chars: Vec<char> = no_strong.chars().collect();
    let mut out = String::with_capacity(no_strong.len());
    for (i, &c) in chars.iter().enumerate() {
        if (c == '*' || c == '_')
            && !(i > 0
                && chars[i - 1].is_alphanumeric()
                && chars.get(i + 1).is_some_and(|n| n.is_alphanumeric()))
        {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

const HTML_FORMAT_TAGS: &[&str] = &[
    "b", "strong", "i", "em", "u", "s", "strike", "del", "ins", "mark", "small", "big", "sub",
    "sup", "code", "kbd", "samp", "var", "tt", "span", "font", "q", "abbr", "cite", "pre",
];

fn strip_html_tags(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<'
            && let Some((name, end)) = html_tag_at(&chars, i)
        {
            let lower = name.to_ascii_lowercase();
            if lower == "br" {
                out.push(' ');
                i = end;
                continue;
            }
            if HTML_FORMAT_TAGS.contains(&lower.as_str()) {
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn html_tag_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start + 1;
    if chars.get(i) == Some(&'/') {
        i += 1;
    }
    let name_start = i;
    while i < chars.len() && chars[i].is_ascii_alphanumeric() {
        i += 1;
    }
    if i == name_start {
        return None;
    }
    let name: String = chars[name_start..i].iter().collect();
    while i < chars.len() && chars[i] != '>' {
        if chars[i] == '<' {
            return None;
        }
        i += 1;
    }
    if chars.get(i) == Some(&'>') {
        Some((name, i + 1))
    } else {
        None
    }
}

fn is_link_char(c: char) -> bool {
    matches!(c, '-' | '.' | '=' | '<' | '>')
}

fn parse_link(
    chars: &[char],
    start: usize,
) -> Option<(Head, Head, LineKind, Option<String>, usize)> {
    let mut i = skip_spaces(chars, start);
    let mut left = Head::None;
    if let Some(&c) = chars.get(i)
        && matches!(c, 'o' | 'x')
        && matches!(chars.get(i + 1), Some('-' | '.' | '='))
    {
        left = if c == 'o' { Head::Circle } else { Head::Cross };
        i += 1;
    }
    let op_start = i;
    while i < chars.len() && matches!(chars[i], '-' | '.' | '=' | '<' | '>') {
        i += 1;
    }
    if i == op_start {
        return None;
    }
    let op1: String = chars[op_start..i].iter().collect();
    if left == Head::None && op1.starts_with('<') {
        left = Head::Arrow;
    }
    let mut line = line_kind(&op1);
    let mut right = if op1.contains('>') {
        Head::Arrow
    } else {
        Head::None
    };
    if right == Head::None
        && let Some((head, ni)) = trailing_head(chars, i)
    {
        right = head;
        i = ni;
    }

    if chars.get(i) == Some(&'|') {
        i += 1;
        let l_start = i;
        while i < chars.len() && chars[i] != '|' {
            i += 1;
        }
        let label = clean_label(&chars[l_start..i].iter().collect::<String>());
        if chars.get(i) == Some(&'|') {
            i += 1;
        }
        return Some((left, right, line, non_empty(label), i));
    }

    if right == Head::None {
        let text_start = skip_spaces(chars, i);
        let mut j = text_start;
        while j < chars.len() && !is_link_char(chars[j]) {
            j += 1;
        }
        if j < chars.len() && j > text_start && matches!(chars[j], '-' | '.' | '=' | '>') {
            let text: String = chars[text_start..j].iter().collect();
            let op2_start = j;
            while j < chars.len() && is_link_char(chars[j]) {
                j += 1;
            }
            let op2: String = chars[op2_start..j].iter().collect();
            right = if op2.contains('>') {
                Head::Arrow
            } else if let Some((head, nj)) = trailing_head(chars, j) {
                j = nj;
                head
            } else {
                Head::None
            };
            if line == LineKind::Solid {
                line = line_kind(&op2);
            }
            return Some((left, right, line, non_empty(clean_label(&text)), j));
        }
    }

    Some((left, right, line, None, i))
}

fn line_kind(op: &str) -> LineKind {
    if op.contains('=') {
        LineKind::Thick
    } else if op.contains('.') {
        LineKind::Dotted
    } else {
        LineKind::Solid
    }
}

fn trailing_head(chars: &[char], i: usize) -> Option<(Head, usize)> {
    let head = match chars.get(i) {
        Some('o') => Head::Circle,
        Some('x') => Head::Cross,
        _ => return None,
    };
    match chars.get(i + 1) {
        None | Some(' ') | Some('\t') | Some('|') | Some('&') | Some(';') => Some((head, i + 1)),
        _ => None,
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}
