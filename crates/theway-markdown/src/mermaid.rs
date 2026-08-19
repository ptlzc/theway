//! Self-contained terminal renderer for Mermaid diagrams.
//!
//! Renders `graph`/`flowchart`, `sequenceDiagram`, and `stateDiagram` blocks
//! as Unicode box-drawing art; unsupported diagram types fall back to the raw
//! source in a framed box.

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Theme-derived styles used when painting a diagram.
///
/// [`Default`] renders an unstyled (terminal-default) diagram; consumers can
/// override individual roles (e.g. the TUI DAG band colors its borders).
#[derive(Clone, Copy, Default)]
pub struct MermaidStyles {
    pub border: Style,
    pub node_text: Style,
    pub edge: Style,
    pub edge_label: Style,
    pub title: Style,
}

/// Rendered diagram: styled lines for the TUI and plain lines for ANSI output.
///
/// `fallback` is `true` when the source could not be laid out as a diagram
/// (unsupported type, unparseable, or over-wide) and the framed source box
/// was emitted instead — callers that require a real diagram (e.g. the DAG
/// band) can use it to pick a different presentation.
pub struct MermaidArt {
    pub styled_lines: Vec<Line<'static>>,
    pub plain_lines: Vec<String>,
    /// Framed-source fallback rather than a laid-out diagram.
    pub fallback: bool,
}

const MAX_LABEL: usize = 28;
const PAD: usize = 1;
const GAP_X: usize = 3;
const GAP_Y: usize = 2;
/// Node labels wrap to at most this many display columns per line, and at most
/// this many lines (overflow is truncated with an ellipsis).
const WRAP_WIDTH: usize = 24;
const MAX_LINES: usize = 4;
/// Identifier-boundary characters preferred as break points when a single word
/// is too wide to fit, so it is not sliced mid-segment.
/// Mirrors `TOKEN_BREAK_CHARS` in `third_party/mermaid-to-svg/src/text_wrap.rs`;
/// the two renderers are deliberately independent, so keep these two in sync.
const LABEL_BREAK_CHARS: [char; 4] = ['_', '-', '.', '/'];
/// Sentinel marking the trailing column of a wide glyph (never emitted).
const CONT: char = '\u{0}';
const MAX_NODES: usize = 128;
const MAX_EDGES: usize = 512;
const MAX_GROUPS: usize = 24;
const MAX_GROUP_DEPTH: usize = 6;
const MAX_CANVAS_CELLS: usize = 1 << 21;

fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

#[derive(Clone, Copy)]
enum Oversize {
    Width,
    Cells,
}

/// Render a mermaid source block, or `None` for blank input.
pub fn render_mermaid_art(
    src: &str,
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Option<MermaidArt> {
    if src.trim().is_empty() {
        return None;
    }

    let outcome: Option<Result<MermaidArt, Oversize>> = parse_graph(src)
        .map(|graph| {
            if graph.groups.is_empty() {
                layout_flowchart(&graph, styles, max_width)
            } else {
                render_grouped(&graph, styles, max_width)
            }
        })
        .or_else(|| parse_state(src).map(|graph| layout_flowchart(&graph, styles, max_width)))
        .or_else(|| {
            parse_class(src).map(|(graph, infos)| render_class(&graph, &infos, styles, max_width))
        })
        .or_else(|| {
            parse_er(src).map(|(graph, infos)| render_class(&graph, &infos, styles, max_width))
        })
        .or_else(|| parse_sequence(src).map(|seq| layout_sequence(&seq, styles, max_width)));

    let too_wide = match outcome {
        Some(Ok(art)) => return Some(art),
        Some(Err(Oversize::Width)) => true,
        Some(Err(Oversize::Cells)) | None => false,
    };
    Some(fallback(src, styles, max_width, too_wide))
}

/// Internal alias kept for the markdown parse path in `parse.rs` (same
/// behavior as [`render_mermaid_art`]).
pub(crate) fn render(
    src: &str,
    styles: &MermaidStyles,
    max_width: Option<usize>,
) -> Option<MermaidArt> {
    render_mermaid_art(src, styles, max_width)
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Rect,
    Round,
    Diamond,
}

struct Node {
    label: String,
    shape: Shape,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Head {
    None,
    Arrow,
    Circle,
    Cross,
    Triangle,
    DiamondFill,
    DiamondOpen,
}

#[derive(Clone, Copy, PartialEq)]
enum LineKind {
    Solid,
    Dotted,
    Thick,
}

struct Edge {
    from: usize,
    to: usize,
    label: Option<String>,
    head_to: Head,
    head_from: Head,
    line: LineKind,
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Down,
    Up,
    Right,
    Left,
}

struct Group {
    id: String,
    label: String,
    parent: Option<usize>,
}

struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    index: HashMap<String, usize>,
    groups: Vec<Group>,
    node_group: Vec<Option<usize>>,
    cur_group: Option<usize>,
    over_cap: bool,
    dir: Dir,
}

impl Graph {
    fn node_index(&mut self, id: &str, label: Option<&str>, shape: Shape) -> Option<usize> {
        if let Some(&i) = self.index.get(id) {
            if let Some(label) = label {
                self.nodes[i].label = label.to_string();
                self.nodes[i].shape = shape;
            }
            return Some(i);
        }
        if self.nodes.len() >= MAX_NODES {
            self.over_cap = true;
            return None;
        }
        let label = label.unwrap_or(id).to_string();
        self.index.insert(id.to_string(), self.nodes.len());
        self.nodes.push(Node { label, shape });
        self.node_group.push(self.cur_group);
        Some(self.nodes.len() - 1)
    }

    fn node_label(&mut self, id: &str, label: &str) -> Option<usize> {
        if let Some(&i) = self.index.get(id) {
            self.nodes[i].label = label.to_string();
            return Some(i);
        }
        self.node_index(id, Some(label), Shape::Round)
    }
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/mermaid/graph_parse.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/mermaid/diagram_parse.rs"
));

struct Canvas {
    w: usize,
    h: usize,
    ch: Vec<char>,
    cls: Vec<Cls>,
    mask: Vec<u8>,
    style: Vec<u8>,
    occupied: Vec<bool>,
    cur_style: u8,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Self {
            w,
            h,
            ch: vec![' '; n],
            cls: vec![Cls::Empty; n],
            mask: vec![0; n],
            style: vec![0; n],
            occupied: vec![false; n],
            cur_style: STY_SOLID,
        }
    }

    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    fn set(&mut self, x: usize, y: usize, c: char, cls: Cls) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        self.ch[i] = c;
        self.cls[i] = cls;
    }

    fn add_bits(&mut self, x: usize, y: usize, bits: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        if self.occupied[i] {
            return;
        }
        self.mask[i] |= bits;
        self.style[i] |= self.cur_style;
        if self.cls[i] != Cls::Border {
            self.cls[i] = Cls::Edge;
        }
    }

    fn blit(&mut self, sub: &Canvas, ox: usize, oy: usize) {
        for sy in 0..sub.h {
            for sx in 0..sub.w {
                let (x, y) = (ox + sx, oy + sy);
                if x >= self.w || y >= self.h {
                    continue;
                }
                let si = sub.idx(sx, sy);
                let di = self.idx(x, y);
                self.ch[di] = sub.ch[si];
                self.cls[di] = sub.cls[si];
                self.style[di] = sub.style[si];
                self.occupied[di] = true;
            }
        }
    }

    fn junction(&mut self, x: usize, y: usize, bits: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        self.mask[i] |= bits;
        if self.cls[i] != Cls::Border {
            self.cls[i] = Cls::Edge;
        }
    }

    fn seg_v(&mut self, x: usize, y0: usize, y1: usize) {
        let (a, b) = (y0.min(y1), y0.max(y1));
        for y in a..=b {
            let mut bits = 0;
            if y > a {
                bits |= U;
            }
            if y < b {
                bits |= D;
            }
            self.add_bits(x, y, bits);
        }
    }

    fn seg_h(&mut self, y: usize, x0: usize, x1: usize) {
        let (a, b) = (x0.min(x1), x0.max(x1));
        for x in a..=b {
            let mut bits = 0;
            if x > a {
                bits |= L;
            }
            if x < b {
                bits |= R;
            }
            self.add_bits(x, y, bits);
        }
    }

    fn finalize_mask(&mut self) {
        for i in 0..self.ch.len() {
            if self.mask[i] != 0 && self.ch[i] == ' ' {
                let c = mask_char(self.mask[i]);
                self.ch[i] = match self.style[i] {
                    STY_DOT => dotted_char(c),
                    STY_THICK => thick_char(c),
                    _ => c,
                };
            }
        }
    }

    /// Mirror top-to-bottom for `BT` (rows reorder; within-row text is
    /// unaffected, so labels stay readable). Box-drawing glyphs flip too.
    fn flip_vertical(&mut self) {
        for y in 0..self.h / 2 {
            let y2 = self.h - 1 - y;
            for x in 0..self.w {
                let (i, j) = (self.idx(x, y), self.idx(x, y2));
                self.ch.swap(i, j);
                self.cls.swap(i, j);
            }
        }
        for c in self.ch.iter_mut() {
            *c = flip_glyph_v(*c);
        }
    }

    /// Mirror left-to-right for `RL`. Mirroring reverses each row, so after
    /// flipping glyphs we reverse each text/label run back to reading order.
    fn flip_horizontal(&mut self) {
        for y in 0..self.h {
            for x in 0..self.w / 2 {
                let x2 = self.w - 1 - x;
                let (i, j) = (self.idx(x, y), self.idx(x2, y));
                self.ch.swap(i, j);
                self.cls.swap(i, j);
            }
        }
        for c in self.ch.iter_mut() {
            *c = flip_glyph_h(*c);
        }
        for y in 0..self.h {
            let mut x = 0;
            while x < self.w {
                let cls = self.cls[self.idx(x, y)];
                if cls == Cls::Text || cls == Cls::EdgeLabel {
                    let start = self.idx(x, y);
                    while x < self.w && self.cls[self.idx(x, y)] == cls {
                        x += 1;
                    }
                    let end = self.idx(x, y);
                    self.ch[start..end].reverse();
                } else {
                    x += 1;
                }
            }
        }
    }

    fn to_lines(&self, styles: &MermaidStyles) -> (Vec<Line<'static>>, Vec<String>) {
        let mut styled = Vec::with_capacity(self.h);
        let mut plain = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let mut last = self.w;
            for x in (0..self.w).rev() {
                let c = self.ch[self.idx(x, y)];
                if c != ' ' && c != CONT {
                    last = x + 1;
                    break;
                }
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut plain_row = String::new();
            let mut run = String::new();
            let mut run_cls = Cls::Empty;
            for x in 0..last {
                let i = self.idx(x, y);
                let c = self.ch[i];
                if c == CONT {
                    continue;
                }
                let cls = self.cls[i];
                plain_row.push(c);
                if cls != run_cls && !run.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut run),
                        style_for(run_cls, styles),
                    ));
                }
                run_cls = cls;
                run.push(c);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, style_for(run_cls, styles)));
            }
            styled.push(Line::from(spans));
            plain.push(plain_row.trim_end().to_string());
        }
        (styled, plain)
    }
}

fn style_for(cls: Cls, styles: &MermaidStyles) -> Style {
    match cls {
        Cls::Empty => Style::default(),
        Cls::Border => styles.border,
        Cls::Text => styles.node_text,
        Cls::Edge => styles.edge,
        Cls::EdgeLabel => styles.edge_label,
    }
}

fn mask_char(mask: u8) -> char {
    match mask {
        0 => ' ',
        m if m == U || m == D || m == U | D => '│',
        m if m == L || m == R || m == L | R => '─',
        m if m == D | R => '┌',
        m if m == D | L => '┐',
        m if m == U | R => '└',
        m if m == U | L => '┘',
        m if m == U | D | R => '├',
        m if m == U | D | L => '┤',
        m if m == D | L | R => '┬',
        m if m == U | L | R => '┴',
        _ => '┼',
    }
}

fn dotted_char(c: char) -> char {
    match c {
        '─' => '╌',
        '│' => '╎',
        other => other,
    }
}

fn thick_char(c: char) -> char {
    match c {
        '─' => '━',
        '│' => '┃',
        '┌' => '┏',
        '┐' => '┓',
        '└' => '┗',
        '┘' => '┛',
        '├' => '┣',
        '┤' => '┫',
        '┬' => '┳',
        '┴' => '┻',
        '┼' => '╋',
        other => other,
    }
}

fn flip_glyph_v(c: char) -> char {
    match c {
        '┌' => '└',
        '└' => '┌',
        '┐' => '┘',
        '┘' => '┐',
        '┏' => '┗',
        '┗' => '┏',
        '┓' => '┛',
        '┛' => '┓',
        '╭' => '╰',
        '╰' => '╭',
        '╮' => '╯',
        '╯' => '╮',
        '┬' => '┴',
        '┴' => '┬',
        '┳' => '┻',
        '┻' => '┳',
        '▼' => '▲',
        '▲' => '▼',
        '▽' => '△',
        '△' => '▽',
        other => other,
    }
}

fn flip_glyph_h(c: char) -> char {
    match c {
        '┌' => '┐',
        '┐' => '┌',
        '└' => '┘',
        '┘' => '└',
        '┏' => '┓',
        '┓' => '┏',
        '┗' => '┛',
        '┛' => '┗',
        '╭' => '╮',
        '╮' => '╭',
        '╰' => '╯',
        '╯' => '╰',
        '├' => '┤',
        '┤' => '├',
        '┣' => '┫',
        '┫' => '┣',
        '▶' => '◄',
        '◄' => '▶',
        '▷' => '◁',
        '◁' => '▷',
        other => other,
    }
}

struct Placed {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    cx: usize,
    cy: usize,
    rank: usize,
}

struct NodeSizes {
    box_w: Vec<usize>,
    box_h: Vec<usize>,
    lay_w: Vec<usize>,
    lay_h: Vec<usize>,
    extra_h: Vec<usize>,
    self_label_w: Vec<usize>,
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/mermaid/layout.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/mermaid/routing.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/mermaid/sequence.rs"
));

fn first_word(src: &str) -> String {
    src.split_whitespace()
        .next()
        .unwrap_or("diagram")
        .to_string()
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("mermaid/unit");
