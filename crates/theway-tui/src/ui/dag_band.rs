//! DAG status band (issue #38): compact live view of DAG runs rendered
//! between the feed and the composer busy band while `latest.dags` is
//! non-empty.
//!
//! Each run renders as a bordered box. The header (`dag-2 · name · done/total
//! · c/s 84`, with a mini rainbow spinner while any node runs) is embedded in
//! the top border; every node takes one text row — state glyph + id, a dim
//! `← dep` annotation listing the node's dependencies, and the error summary
//! for failed/cancelled nodes — capped at [`MAX_NODE_ROWS`] rows with a
//! `… N more` tail row inside the box.
//!
//! Box widths adapt to their content (header and widest node row). When two
//! runs fit side by side within the band width they are placed next to each
//! other and the band takes the height of the taller box; otherwise boxes
//! stack vertically. Runs beyond [`MAX_RUNS`] collapse into a `… N more`
//! line. Run-level throughput reuses the busy-band [`CpsMeter`]: one meter
//! per run samples the cumulative `sum(node.output_tokens)` each tick over a
//! 1 s sliding window, and the same cps → step-delay mapping drives the mini
//! spinner speed.

use std::collections::HashMap;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use theway_transport::wire::{WireDagNodeSnapshot, WireDagRunSnapshot};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::pixel_loader;
use super::stats::CpsMeter;

/// Maximum runs rendered; extra runs collapse into the `… N more` line.
pub const MAX_RUNS: usize = 2;
/// Node rows per run before the box appends a `… N more` tail row.
pub const MAX_NODE_ROWS: usize = 3;
/// Spinner animation cadence — one tick per event-loop frame interval,
/// matching `SPINNER_TICK_MS` in `ui/mod.rs`.
const TICK_MS: u64 = 10;
/// Error summary length after a failed/cancelled node (chars).
const ERROR_SUMMARY_CHARS: usize = 20;
/// Node separator inside the run header.
const SEPARATOR: &str = " · ";
/// Horizontal gap between side-by-side boxes (columns).
const BOX_GAP: u16 = 1;
/// Box border padding: `╭─ ` … `╮` and `│ ` … ` │` (4 columns total).
const BOX_PAD: u16 = 4;

/// State glyph, color, and modifier for a node status string (design §8.2
/// table). Unknown statuses render as pending. Colors come from the
/// `[dag_band]` theme table (issue #31); defaults equal the pre-theme
/// hardcoded values.
#[must_use]
pub fn node_style(status: &str, band: &crate::ui::theme::DagBandStyle) -> (char, Color, Modifier) {
    match status {
        "ready" => ('▸', band.pending, Modifier::empty()),
        "running" => ('▶', band.running, Modifier::empty()),
        "succeeded" => ('✓', band.ok, Modifier::empty()),
        "failed" => ('✗', band.failed, Modifier::empty()),
        "cancelled" => ('×', band.cancelled, Modifier::CROSSED_OUT),
        "skipped" => ('↷', band.skipped, Modifier::empty()),
        _ => ('·', band.fg, Modifier::empty()),
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
/// to fit `width` (display cells). Callers embedding the header into a box
/// top border pass a generous width and fit it separately.
#[must_use]
pub fn run_header_line(
    run: &WireDagRunSnapshot,
    cps: f64,
    tick: u64,
    width: u16,
    band: &crate::ui::theme::DagBandStyle,
) -> Line<'static> {
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
        spans.push(Span::styled(SEPARATOR, separator_style(band)));
        spans.push(Span::styled(
            truncate_to_width(&run.name, name_budget),
            Style::default().fg(band.title),
        ));
    }
    spans.push(Span::styled(tail, Style::default().fg(band.fg)));
    Line::from(spans)
}

fn separator_style(band: &crate::ui::theme::DagBandStyle) -> Style {
    Style::default().fg(band.edge)
}

/// One node's text row: state glyph + id in the state color (cancelled also
/// strikes through), then a dim `← dep1, dep2` dependency annotation, then
/// the error summary for failed/cancelled nodes. The row is fitted to
/// `max_w` display cells: trailing annotation spans drop first, then the
/// id truncates with an ellipsis.
fn node_line(
    node: &WireDagNodeSnapshot,
    band: &crate::ui::theme::DagBandStyle,
    max_w: usize,
) -> Line<'static> {
    let (glyph, color, modifier) = node_style(&node.status, band);
    let mut spans = vec![Span::styled(
        format!("{glyph} {}", node.id),
        Style::default().fg(color).add_modifier(modifier),
    )];
    if !node.depends_on.is_empty() {
        spans.push(Span::styled(
            format!(" ← {}", node.depends_on.join(", ")),
            Style::default().fg(band.fg).add_modifier(Modifier::DIM),
        ));
    }
    if matches!(node.status.as_str(), "failed" | "cancelled")
        && let Some(error) = node.error.as_deref()
        && !error.trim().is_empty()
    {
        spans.push(Span::styled(
            format!(" {}", error_summary(error)),
            Style::default().fg(band.fg),
        ));
    }
    // Fit to `max_w`: drop trailing annotation/error spans, then truncate
    // the glyph+id span itself.
    let mut total = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum::<usize>();
    while total > max_w && spans.len() > 1 {
        spans.pop();
        total = spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum::<usize>();
    }
    if total > max_w {
        let main = spans.remove(0);
        spans.push(Span::styled(
            truncate_to_width(main.content.as_ref(), max_w),
            main.style,
        ));
    }
    Line::from(spans)
}

/// The run's node text rows, one per node and capped at [`MAX_NODE_ROWS`];
/// overflow appends a `… N more` tail row (also width-fitted).
fn run_node_lines(
    run: &WireDagRunSnapshot,
    band: &crate::ui::theme::DagBandStyle,
    max_w: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = run
        .nodes
        .iter()
        .take(MAX_NODE_ROWS)
        .map(|node| node_line(node, band, max_w))
        .collect();
    if run.nodes.len() > MAX_NODE_ROWS {
        lines.push(Line::styled(
            truncate_to_width(
                &format!("… {} more", run.nodes.len() - MAX_NODE_ROWS),
                max_w,
            ),
            Style::default().fg(band.fg),
        ));
    }
    lines
}

fn line_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// A laid-out box: header line (embedded in the top border), node rows,
/// and its position/size relative to the band origin.
#[derive(Clone)]
struct PlacedBox {
    header: Line<'static>,
    lines: Vec<Line<'static>>,
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

/// Fit the header into `max_w` display cells: when it overflows, the whole
/// line flattens and truncates with an ellipsis (rare — only on very narrow
/// bands, where the colored spans degrade to the plain band foreground).
fn fit_header(
    header: &Line<'static>,
    max_w: usize,
    band: &crate::ui::theme::DagBandStyle,
) -> Line<'static> {
    if line_width(header) <= max_w {
        return header.clone();
    }
    let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
    Line::styled(
        truncate_to_width(&text, max_w),
        Style::default().fg(band.fg),
    )
}

/// Compute the box layout for `dags` within `width` columns: box widths
/// adapt to content, two runs go side by side when they fit (returned as
/// `side_by_side`), otherwise boxes stack vertically. `meters`/`tick` only
/// shape the header spinner glyph, never the widths.
fn layout_boxes(
    dags: &[WireDagRunSnapshot],
    width: u16,
    meters: &HashMap<String, CpsMeter>,
    tick: u64,
    band: &crate::ui::theme::DagBandStyle,
) -> (Vec<PlacedBox>, bool) {
    let shown = dags.len().min(MAX_RUNS);
    let text_w = usize::from(width.saturating_sub(BOX_PAD));
    struct Raw {
        header: Line<'static>,
        lines: Vec<Line<'static>>,
        w: u16,
    }
    let raws: Vec<Raw> = dags[..shown]
        .iter()
        .map(|run| {
            let cps = meters.get(&run.id).map(CpsMeter::cps).unwrap_or(0.0);
            let header = run_header_line(run, cps, tick, u16::MAX, band);
            let header_w = line_width(&header);
            // First pass at the full band text width: enough to decide the
            // box width. Once `w` is known the rows are re-fitted to the
            // box's actual inner width below.
            let lines = run_node_lines(run, band, text_w);
            let node_w = lines.iter().map(line_width).max().unwrap_or(0);
            let content_w = header_w.max(node_w);
            let w = u16::try_from(content_w)
                .unwrap_or(u16::MAX)
                .saturating_add(BOX_PAD)
                .min(width);
            let lines = run_node_lines(run, band, usize::from(w.saturating_sub(BOX_PAD)));
            Raw { header, lines, w }
        })
        .collect();
    let side_by_side = raws.len() >= 2 && raws[0].w + BOX_GAP + raws[1].w <= width;
    let mut boxes = Vec::with_capacity(raws.len());
    if side_by_side {
        let h = raws
            .iter()
            .map(|raw| raw.lines.len() as u16 + 2)
            .max()
            .unwrap_or(2);
        let second_x = raws[0].w + BOX_GAP;
        for (i, raw) in raws.iter().enumerate() {
            boxes.push(PlacedBox {
                header: raw.header.clone(),
                lines: raw.lines.clone(),
                x: if i == 0 { 0 } else { second_x },
                y: 0,
                w: raw.w,
                h,
            });
        }
    } else {
        let mut y = 0u16;
        for raw in &raws {
            let h = raw.lines.len() as u16 + 2;
            boxes.push(PlacedBox {
                header: raw.header.clone(),
                lines: raw.lines.clone(),
                x: 0,
                y,
                w: raw.w,
                h,
            });
            y += h;
        }
    }
    (boxes, side_by_side)
}

/// Band height (rows) for `dags` at `width`: side-by-side boxes take the
/// taller height, stacked boxes sum, and runs beyond [`MAX_RUNS`] add one
/// `… N more` row. Zero with no runs.
///
/// Issue #98: a run is "live" until it reaches a terminal state. The band
/// (and the idle tick that animates it) only cares about live runs — once
/// every run is terminal the band closes by itself.
#[must_use]
pub fn has_live_runs(dags: &[WireDagRunSnapshot]) -> bool {
    dags.iter()
        .any(|run| !matches!(run.status.as_str(), "succeeded" | "failed" | "cancelled"))
}

/// Band height (rows) for `dags` at `width`: side-by-side boxes take the
/// taller height, stacked boxes sum, and runs beyond [`MAX_RUNS`] add one
/// `… N more` row. Zero with no runs.
#[must_use]
pub fn band_rows(dags: &[WireDagRunSnapshot], width: u16) -> u16 {
    if dags.is_empty() {
        return 0;
    }
    let (boxes, _) = layout_boxes(
        dags,
        width,
        &HashMap::new(),
        0,
        &crate::ui::theme::DagBandStyle::default(),
    );
    let mut rows = boxes.iter().map(|b| b.y + b.h).max().unwrap_or(0);
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

/// Draw one bordered box at `(x, y)` (absolute buffer coordinates):
/// `╭─ header ─╮` top border with the header embedded, one `│ … │` row per
/// node line, `╰───╯` bottom border.
fn draw_box(
    buf: &mut Buffer,
    area: Rect,
    x: u16,
    y: u16,
    pb: &PlacedBox,
    band: &crate::ui::theme::DagBandStyle,
) {
    let edge = separator_style(band);
    let inner_w = usize::from(pb.w.saturating_sub(BOX_PAD));
    // Top border with the embedded header: `╭─ {header} ─╮` — the leading
    // space keeps the header visually detached from the corner.
    let header = fit_header(&pb.header, inner_w, band);
    let header_w = line_width(&header);
    let fill = usize::from(pb.w).saturating_sub(3 + header_w + 1);
    let mut top_spans = vec![Span::styled("╭─ ", edge)];
    top_spans.extend(header.spans);
    top_spans.push(Span::styled(format!("{}╮", "─".repeat(fill)), edge));
    buf.set_line(x, y, &Line::from(top_spans), pb.w);
    // Node rows: every row between top and bottom gets the `│` border, so
    // boxes placed side by side align flush at the bottom even when one has
    // fewer nodes — the shorter box renders empty bordered rows up to the
    // shared height.
    let rows = usize::from(pb.h.saturating_sub(2));
    for i in 0..rows {
        let row_y = y + 1 + i as u16;
        if row_y >= area.bottom() {
            break;
        }
        let mut spans = vec![Span::styled("│ ", edge)];
        match pb.lines.get(i) {
            Some(line) => {
                let line_w = line_width(line);
                let pad = inner_w.saturating_sub(line_w);
                spans.extend(line.spans.clone());
                spans.push(Span::raw(" ".repeat(pad)));
            }
            None => spans.push(Span::raw(" ".repeat(inner_w))),
        }
        spans.push(Span::styled(" │", edge));
        buf.set_line(x, row_y, &Line::from(spans), pb.w);
    }
    // Bottom border.
    let bottom_y = y + pb.h - 1;
    if bottom_y < area.bottom() {
        buf.set_line(
            x,
            bottom_y,
            &Line::from(Span::styled(
                format!("╰{}╯", "─".repeat(usize::from(pb.w) - 2)),
                edge,
            )),
            pb.w,
        );
    }
}

/// Render the DAG band into `buf` at `area`: bordered text boxes per run,
/// side by side when they fit, then a `… N more` row for runs beyond
/// [`MAX_RUNS`]. Pure: reads only `dags`, `meters`, and `tick`.
pub fn render_dag_band(
    buf: &mut Buffer,
    area: Rect,
    dags: &[WireDagRunSnapshot],
    meters: &HashMap<String, CpsMeter>,
    tick: u64,
    band: &crate::ui::theme::DagBandStyle,
) {
    if dags.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let (boxes, _) = layout_boxes(dags, area.width, meters, tick, band);
    for pb in &boxes {
        let x = area.x + pb.x;
        let y = area.y + pb.y;
        if y >= area.bottom() {
            continue;
        }
        draw_box(buf, area, x, y, pb, band);
    }
    if dags.len() > MAX_RUNS {
        let more_y = area.y + boxes.iter().map(|b| b.y + b.h).max().unwrap_or(0);
        if more_y < area.bottom() {
            let more = Line::styled(
                format!("… {} more", dags.len() - MAX_RUNS),
                separator_style(band),
            );
            buf.set_line(area.x, more_y, &more, area.width);
        }
    }
}

#[cfg(test)]
#[path = "dag_band_tests.rs"]
mod tests;
