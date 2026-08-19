//! DAG status band (issue #38): compact live view of DAG runs rendered
//! between the feed and the composer busy band while `latest.dags` is
//! non-empty.
//!
//! Each run gets a header line (`dag-2 · name · done/total · c/s 84`, with
//! a mini rainbow spinner while any node runs) plus node rows: wire-order
//! state glyphs separated by ` · `, wrapping to the band width and capped
//! at three rows; runs beyond the first two collapse into a `… N more`
//! line. Run-level throughput reuses the busy-band [`CpsMeter`]: one meter
//! per run samples the cumulative `sum(node.output_tokens)` each tick over
//! a 1 s sliding window, and the same cps → step-delay mapping drives the
//! mini spinner speed.

use std::collections::HashMap;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use theway_markdown::{MermaidStyles, render_mermaid_art};
use theway_transport::wire::{WireDagNodeSnapshot, WireDagRunSnapshot};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::pixel_loader;
use super::stats::CpsMeter;

/// Maximum runs rendered; extra runs collapse into the `… N more` line.
pub const MAX_RUNS: usize = 2;
/// Node rows per run before the band truncates (plus one header row).
pub const MAX_NODE_ROWS: usize = 3;
/// Spinner animation cadence — one tick per event-loop frame interval,
/// matching `SPINNER_TICK_MS` in `ui/mod.rs`.
const TICK_MS: u64 = 10;
/// Error summary length after a failed/cancelled node (chars).
const ERROR_SUMMARY_CHARS: usize = 20;
/// Node separator.
const SEPARATOR: &str = " · ";
/// Left indent for the header line and the `… N more` line.
const HEADER_INDENT: u16 = 1;
/// Left indent for node rows.
const NODE_INDENT: u16 = 3;

/// State glyph, color, and modifier for a node status string (design §8.2
/// table). Unknown statuses render as pending.
#[must_use]
pub fn node_style(status: &str) -> (char, Color, Modifier) {
    match status {
        "ready" => ('▸', Color::Yellow, Modifier::empty()),
        "running" => ('▶', Color::Cyan, Modifier::empty()),
        "succeeded" => ('✓', Color::Green, Modifier::empty()),
        "failed" => ('✗', Color::Red, Modifier::empty()),
        "cancelled" => ('×', Color::DarkGray, Modifier::CROSSED_OUT),
        "skipped" => ('↷', Color::Gray, Modifier::empty()),
        _ => ('·', Color::DarkGray, Modifier::empty()),
    }
}

/// `(done, total)` progress: succeeded + skipped nodes over node count.
#[must_use]
pub fn run_progress(run: &WireDagRunSnapshot) -> (usize, usize) {
    let done = run
        .nodes
        .iter()
        .filter(|node| matches!(node.status.as_str(), "succeeded" | "skipped"))
        .count();
    (done, run.nodes.len())
}

/// One-line error summary for a failed/cancelled node: whitespace
/// flattened, capped at [`ERROR_SUMMARY_CHARS`] chars (truncated to 19 + `…`).
#[must_use]
pub fn error_summary(error: &str) -> String {
    let flat = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= ERROR_SUMMARY_CHARS {
        return flat;
    }
    let mut out: String = flat.chars().take(ERROR_SUMMARY_CHARS - 1).collect();
    out.push('…');
    out
}

/// Truncate `text` to at most `max` display cells, appending `…` when it
/// does not fit (the ellipsis counts against the budget).
fn truncate_to_width(text: &str, max: usize) -> String {
    if UnicodeWidthStr::width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_w > max - 1 {
            break;
        }
        out.push(ch);
        width += ch_w;
    }
    out.push('…');
    out
}

/// Mini one-cell rainbow spinner for a run header: the shared 3×3
/// [`pixel_loader::rainbow_frame`] collapsed into a braille dot pattern
/// (grid columns 0/1 → left braille column, column 2 → right; rows map
/// 1:1). The wavefront step derives from `tick` and `cps` through the same
/// step-delay mapping as the busy band, so throughput drives speed.
#[must_use]
pub fn mini_spinner(tick: u64, cps: f64) -> Span<'static> {
    let step = tick.saturating_mul(TICK_MS) / pixel_loader::step_delay_ms(cps);
    let frame = pixel_loader::rainbow_frame(step, cps);
    let mut bits = 0u8;
    let mut peak = 0usize;
    for (idx, cell) in frame.cells.iter().enumerate() {
        if cell.lit > frame.cells[peak].lit {
            peak = idx;
        }
        if cell.lit <= 0.0 {
            continue;
        }
        let (col, row) = (idx % 3, idx / 3);
        let bit = match (col.min(1), row) {
            (0, 0) => 0x01,
            (0, 1) => 0x02,
            (0, 2) => 0x04,
            (1, 0) => 0x08,
            (1, 1) => 0x10,
            (1, 2) => 0x20,
            _ => 0u8,
        };
        bits |= bit;
    }
    let glyph = char::from_u32(0x2800 + u32::from(bits)).unwrap_or('⠿');
    Span::styled(glyph.to_string(), Style::default().fg(frame.cells[peak].fg))
}

/// Run header: `[spinner ]{id} · {name} · {done}/{total} · c/s {n}`. The
/// mini spinner renders only while any node is running; the name truncates
/// to fit `width` (display cells).
#[must_use]
pub fn run_header_line(run: &WireDagRunSnapshot, cps: f64, tick: u64, width: u16) -> Line<'static> {
    let any_running = run.nodes.iter().any(|node| node.status == "running");
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut prefix_w = 0usize;
    if any_running {
        spans.push(mini_spinner(tick, cps));
        spans.push(Span::raw(" "));
        prefix_w = 2;
    }
    let (done, total) = run_progress(run);
    let tail = format!(" · {done}/{total} · c/s {}", cps.round() as u64);
    let fixed = prefix_w
        + UnicodeWidthStr::width(run.id.as_str())
        + UnicodeWidthStr::width(SEPARATOR)
        + UnicodeWidthStr::width(tail.as_str());
    let name_budget = usize::from(width).saturating_sub(fixed);
    spans.push(Span::styled(
        run.id.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    if !run.name.is_empty() && name_budget > 0 {
        spans.push(Span::styled(SEPARATOR, separator_style()));
        spans.push(Span::styled(
            truncate_to_width(&run.name, name_budget),
            Style::default().fg(Color::Gray),
        ));
    }
    spans.push(Span::styled(tail, Style::default().fg(Color::DarkGray)));
    Line::from(spans)
}

fn separator_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// One node's spans + display width: state glyph + id in the state color
/// (cancelled also strikes through); failed/cancelled nodes append the dim
/// error summary.
fn node_entry(node: &WireDagNodeSnapshot) -> (Vec<Span<'static>>, usize) {
    let (glyph, color, modifier) = node_style(&node.status);
    let mut spans = vec![Span::styled(
        format!("{glyph} {}", node.id),
        Style::default().fg(color).add_modifier(modifier),
    )];
    let mut width = 2 + UnicodeWidthStr::width(node.id.as_str());
    if matches!(node.status.as_str(), "failed" | "cancelled")
        && let Some(error) = node.error.as_deref()
        && !error.trim().is_empty()
    {
        let summary = error_summary(error);
        width += 1 + UnicodeWidthStr::width(summary.as_str());
        spans.push(Span::styled(
            format!(" {summary}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    (spans, width)
}

/// Wrap the run's node entries into rows of at most `width` display cells,
/// ` · `-separated within a row, capped at [`MAX_NODE_ROWS`] rows (overflow
/// entries drop).
fn node_rows(run: &WireDagRunSnapshot, width: usize) -> Vec<Vec<Span<'static>>> {
    let sep_w = UnicodeWidthStr::width(SEPARATOR);
    let mut rows: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    for (spans, w) in run.nodes.iter().map(node_entry) {
        if !cur.is_empty() && cur_w + sep_w + w > width {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
            if rows.len() == MAX_NODE_ROWS {
                return rows;
            }
        }
        if !cur.is_empty() {
            cur.push(Span::styled(SEPARATOR, separator_style()));
            cur_w += sep_w;
        }
        cur_w += w;
        cur.extend(spans);
    }
    if !cur.is_empty() {
        rows.push(cur);
    }
    rows
}

// ── mermaid box diagram (issue #41) ───────────────────────────────────────

/// Flatten a node id into a mermaid identifier: alphanumerics and `_`
/// survive, everything else becomes `_` (mermaid ids are bare words).
fn mermaid_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Flatten a node id for use inside a `["…"]` label: quotes and newlines
/// break the source, so they become `'` and spaces.
fn mermaid_label(id: &str) -> String {
    id.chars()
        .map(|c| match c {
            '"' => '\'',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

/// Synthesize a `graph {direction}` mermaid source for one run: one
/// `id["{glyph} {id}"]` node per wire node plus a `dep --> id` edge for each
/// `depends_on` entry. Node ids are sanitized via [`mermaid_id`]; the
/// direction comes from the run (`TD` when unknown/absent).
#[must_use]
pub fn synthesize_mermaid(run: &WireDagRunSnapshot) -> String {
    let direction = run.direction.to_ascii_uppercase();
    let direction = match direction.as_str() {
        "TD" | "TB" | "BT" | "LR" | "RL" => direction,
        _ => "TD".to_string(),
    };
    let mut src = format!("graph {direction}\n");
    for node in &run.nodes {
        let glyph = node_style(&node.status).0;
        src.push_str(&format!(
            "  {}[\"{glyph} {}\"]\n",
            mermaid_id(&node.id),
            mermaid_label(&node.id)
        ));
    }
    for node in &run.nodes {
        for dep in &node.depends_on {
            src.push_str(&format!(
                "  {} --> {}\n",
                mermaid_id(dep),
                mermaid_id(&node.id)
            ));
        }
    }
    src
}

/// Border/edge styles for the per-run diagram, in the band palette.
fn diagram_styles() -> MermaidStyles {
    MermaidStyles {
        border: separator_style(),
        edge: separator_style(),
        edge_label: Style::default().fg(Color::DarkGray),
        title: Style::default().fg(Color::Gray),
        ..MermaidStyles::default()
    }
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Try to render `run` as a mermaid box diagram that fits `width` columns
/// and `height_budget` rows (excluding the header row). `None` when the run
/// carries no dependency edges (a flat glyph list reads better as text), or
/// the render falls back to the framed source box / is too wide / too tall —
/// callers then fall back to the wrapped text rows.
fn run_diagram(
    run: &WireDagRunSnapshot,
    width: u16,
    height_budget: u16,
) -> Option<Vec<Line<'static>>> {
    if !run.nodes.iter().any(|node| !node.depends_on.is_empty()) {
        return None;
    }
    let src = synthesize_mermaid(run);
    let art = render_mermaid_art(&src, &diagram_styles(), Some(usize::from(width)))?;
    if art.fallback {
        return None;
    }
    if art.styled_lines.len() > usize::from(height_budget)
        || art
            .styled_lines
            .iter()
            .any(|line| line_width(line) > usize::from(width))
    {
        return None;
    }
    Some(art.styled_lines)
}

/// Band height (rows) for `dags` at `width`: each shown run contributes one
/// header row plus its mermaid box diagram (when one renders within the
/// band width) or its wrapped node rows (capped at [`MAX_NODE_ROWS`]), and
/// runs beyond [`MAX_RUNS`] add one `… N more` row. Zero with no runs.
#[must_use]
pub fn band_rows(dags: &[WireDagRunSnapshot], width: u16) -> u16 {
    let shown = dags.len().min(MAX_RUNS);
    let text_width = width.saturating_sub(NODE_INDENT);
    let mut rows: u16 = 0;
    for run in &dags[..shown] {
        rows += 1;
        match run_diagram(run, text_width, u16::MAX) {
            Some(diagram) => rows += diagram.len() as u16,
            None => {
                rows += node_rows(run, usize::from(text_width)).len() as u16;
            }
        }
    }
    if dags.len() > MAX_RUNS {
        rows += 1;
    }
    rows
}

/// Sample each run's cumulative output-token total into its per-run meter
/// and drop meters for runs gone from the snapshot. Called each spinner
/// tick; the 1 s sliding window inside [`CpsMeter`] yields the run's c/s.
pub fn record_meters(meters: &mut HashMap<String, CpsMeter>, dags: &[WireDagRunSnapshot]) {
    record_meters_at(meters, dags, Instant::now());
}

/// [`record_meters`] at an explicit `now` (test seam).
pub fn record_meters_at(
    meters: &mut HashMap<String, CpsMeter>,
    dags: &[WireDagRunSnapshot],
    now: Instant,
) {
    for run in dags {
        let total: u64 = run
            .nodes
            .iter()
            .map(|node| node.output_tokens.unwrap_or(0))
            .sum();
        meters
            .entry(run.id.clone())
            .or_default()
            .record_at(now, total as usize);
    }
    meters.retain(|id, _| dags.iter().any(|run| run.id == *id));
}

/// Render the DAG band into `buf` at `area`: per run one header row then
/// its mermaid box diagram (when one renders within the remaining rows) or
/// the wrapped node rows; a `… N more` row closes runs beyond [`MAX_RUNS`].
/// Pure: reads only `dags`, `meters`, and `tick`.
pub fn render_dag_band(
    buf: &mut Buffer,
    area: Rect,
    dags: &[WireDagRunSnapshot],
    meters: &HashMap<String, CpsMeter>,
    tick: u64,
) {
    if dags.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let mut y = area.y;
    for run in &dags[..dags.len().min(MAX_RUNS)] {
        if y >= area.bottom() {
            break;
        }
        let cps = meters.get(&run.id).map(CpsMeter::cps).unwrap_or(0.0);
        let width = area.width.saturating_sub(HEADER_INDENT);
        let header = run_header_line(run, cps, tick, width);
        buf.set_line(area.x + HEADER_INDENT, y, &header, width);
        y += 1;
        let text_width = area.width.saturating_sub(NODE_INDENT);
        match run_diagram(run, text_width, area.bottom().saturating_sub(y)) {
            Some(diagram) => {
                for line in diagram {
                    if y >= area.bottom() {
                        break;
                    }
                    buf.set_line(area.x + NODE_INDENT, y, &line, text_width);
                    y += 1;
                }
            }
            None => {
                for row in node_rows(run, usize::from(text_width)) {
                    if y >= area.bottom() {
                        break;
                    }
                    let line = Line::from(row);
                    buf.set_line(
                        area.x + NODE_INDENT,
                        y,
                        &line,
                        area.width.saturating_sub(NODE_INDENT),
                    );
                    y += 1;
                }
            }
        }
    }
    if dags.len() > MAX_RUNS && y < area.bottom() {
        let more = Line::styled(
            format!("… {} more", dags.len() - MAX_RUNS),
            separator_style(),
        );
        buf.set_line(
            area.x + HEADER_INDENT,
            y,
            &more,
            area.width.saturating_sub(HEADER_INDENT),
        );
    }
}

#[cfg(test)]
#[path = "dag_band_tests.rs"]
mod tests;
