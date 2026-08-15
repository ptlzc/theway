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
const TICK_MS: u64 = 100;
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
mod tests {
    use super::*;
    use std::time::Duration;

    fn node(id: &str, status: &str) -> WireDagNodeSnapshot {
        WireDagNodeSnapshot {
            id: id.into(),
            agent: "executor-coder".into(),
            status: status.into(),
            depends_on: Vec::new(),
            job_id: None,
            attempt: 1,
            started_at: None,
            completed_at: None,
            error: None,
            input_tokens: None,
            output_tokens: None,
            result: None,
            output_tail: None,
            live_preview: None,
        }
    }

    fn run(id: &str, nodes: Vec<WireDagNodeSnapshot>) -> WireDagRunSnapshot {
        WireDagRunSnapshot {
            id: id.into(),
            name: "demo".into(),
            kind: "dag".into(),
            status: "running".into(),
            fail_fast: false,
            max_concurrency: 4,
            direction: "TD".into(),
            created_at: 0,
            completed_at: None,
            error: None,
            nodes,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn buffer_row(buf: &Buffer, y: u16) -> String {
        let area = *buf.area();
        let mut row = String::new();
        for x in 0..area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        row.trim_end().to_string()
    }

    fn buffer_text(buf: &Buffer) -> String {
        let area = *buf.area();
        (0..area.height)
            .map(|y| buffer_row(buf, y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn find_cell(buf: &Buffer, glyph: &str) -> (u16, u16) {
        let area = *buf.area();
        for y in 0..area.height {
            for x in 0..area.width {
                if buf[(x, y)].symbol() == glyph {
                    return (x, y);
                }
            }
        }
        panic!("glyph {glyph:?} not found in buffer");
    }

    // ── state style table ───────────────────────────────────────────────

    #[test]
    fn node_style_table() {
        assert_eq!(
            node_style("pending"),
            ('·', Color::DarkGray, Modifier::empty())
        );
        assert_eq!(node_style("ready"), ('▸', Color::Yellow, Modifier::empty()));
        assert_eq!(node_style("running"), ('▶', Color::Cyan, Modifier::empty()));
        assert_eq!(
            node_style("succeeded"),
            ('✓', Color::Green, Modifier::empty())
        );
        assert_eq!(node_style("failed"), ('✗', Color::Red, Modifier::empty()));
        assert_eq!(
            node_style("cancelled"),
            ('×', Color::DarkGray, Modifier::CROSSED_OUT)
        );
        assert_eq!(node_style("skipped"), ('↷', Color::Gray, Modifier::empty()));
        // Unknown statuses fall back to pending.
        assert_eq!(node_style("bogus"), node_style("pending"));
    }

    // ── truncation ──────────────────────────────────────────────────────

    #[test]
    fn error_summary_flattens_and_truncates() {
        assert_eq!(error_summary("boom"), "boom");
        assert_eq!(error_summary("line one\nline  two"), "line one line two");
        let long = "x".repeat(40);
        let summary = error_summary(&long);
        assert_eq!(summary.chars().count(), ERROR_SUMMARY_CHARS);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn header_name_truncates_to_width() {
        let mut fixture = run("dag-2", vec![node("a", "pending")]);
        fixture.name = "issue-38-tui-polish".into();
        let line = run_header_line(&fixture, 0.0, 0, 30);
        let text = line_text(&line);
        assert!(UnicodeWidthStr::width(text.as_str()) <= 30, "{text}");
        assert!(text.starts_with("dag-2 · "), "{text}");
        assert!(text.ends_with(" · 0/1 · c/s 0"), "{text}");
        assert!(text.contains('…'), "{text}");
        // Too narrow for any name: the name drops, header stays intact
        // (id 5 + separator 3 + tail 14 = 22 fixed cells).
        let narrow = run_header_line(&fixture, 0.0, 0, 22);
        assert_eq!(line_text(&narrow), "dag-2 · 0/1 · c/s 0");
    }

    // ── header composition ──────────────────────────────────────────────

    #[test]
    fn header_line_composition() {
        let nodes = vec![
            node("1-a", "succeeded"),
            node("2-b", "skipped"),
            node("3-c", "failed"),
            node("4-d", "pending"),
            node("5-e", "pending"),
            node("6-f", "pending"),
        ];
        let fixture = run("dag-2", nodes);
        let line = run_header_line(&fixture, 84.0, 0, 80);
        // Progress counts succeeded + skipped; c/s rounds.
        assert_eq!(line_text(&line), "dag-2 · demo · 2/6 · c/s 84");
    }

    #[test]
    fn header_mini_spinner_when_running() {
        let fixture = run("dag-2", vec![node("a", "running")]);
        let line = run_header_line(&fixture, 0.0, 0, 80);
        let first = line.spans[0].content.as_ref();
        let ch = first.chars().next().unwrap();
        assert!(('\u{2800}'..='\u{28FF}').contains(&ch), "{first:?}");
        // No spinner without a running node.
        let idle = run("dag-2", vec![node("a", "pending")]);
        let line = run_header_line(&idle, 0.0, 0, 80);
        assert!(line_text(&line).starts_with("dag-2"));
    }

    #[test]
    fn mini_spinner_speed_follows_cps() {
        // tick 1 = 100 ms: idle (250 ms/step) stays at step 0; fast
        // streaming (20 ms/step) is already several steps around.
        assert_eq!(mini_spinner(0, 0.0).content, mini_spinner(1, 0.0).content);
        assert_ne!(
            mini_spinner(1, 0.0).content,
            mini_spinner(1, 10_000.0).content
        );
    }

    // ── node row wrapping ───────────────────────────────────────────────

    #[test]
    fn node_rows_wrap_with_separator_and_cap() {
        let ids: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
        let nodes: Vec<_> = ids.iter().map(|id| node(id, "succeeded")).collect();
        let fixture = run("dag-1", nodes);
        // Entry "✓ n0" = 4 cells; width 15 fits two entries (4+3+4 = 11,
        // a third would need 18) — 10 nodes wrap to 5 rows, capped at 3.
        let rows = node_rows(&fixture, 15);
        assert_eq!(rows.len(), MAX_NODE_ROWS);
        let first: String = rows[0].iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(first, "✓ n0 · ✓ n1");
        // A wide band fits all entries on one row.
        let wide = node_rows(&fixture, 80);
        assert_eq!(wide.len(), 1);
    }

    #[test]
    fn band_rows_counts_runs_and_more_line() {
        assert_eq!(band_rows(&[], 80), 0);
        let one = vec![run("dag-1", vec![node("a", "pending")])];
        assert_eq!(band_rows(&one, 80), 2); // header + one node row
        let three = vec![
            run("dag-1", Vec::new()),
            run("dag-2", Vec::new()),
            run("dag-3", Vec::new()),
        ];
        // Two shown runs (header-only each) + the `… 1 more` line.
        assert_eq!(band_rows(&three, 80), 3);
        // Per-run cap: header + at most three node rows.
        let ids: Vec<String> = (0..50).map(|i| format!("n{i}")).collect();
        let nodes: Vec<_> = ids.iter().map(|id| node(id, "pending")).collect();
        let big = vec![run("dag-1", nodes)];
        assert_eq!(band_rows(&big, 20), 1 + MAX_NODE_ROWS as u16);
    }

    // ── mermaid box diagram (issue #41) ─────────────────────────────────

    /// Three-node dependency chain: `1-explore → 2-impl → 3-verify`.
    fn chained_run() -> WireDagRunSnapshot {
        let explore = node("1-explore", "succeeded");
        let mut imp = node("2-impl", "running");
        imp.depends_on = vec!["1-explore".into()];
        let mut verify = node("3-verify", "pending");
        verify.depends_on = vec!["2-impl".into()];
        run("dag-1", vec![explore, imp, verify])
    }

    #[test]
    fn synthesize_mermaid_source_snapshot() {
        let src = synthesize_mermaid(&chained_run());
        let expected = "graph TD\n  \
            1_explore[\"✓ 1-explore\"]\n  \
            2_impl[\"▶ 2-impl\"]\n  \
            3_verify[\"· 3-verify\"]\n  \
            1_explore --> 2_impl\n  \
            2_impl --> 3_verify\n";
        assert_eq!(src, expected);
    }

    #[test]
    fn synthesize_mermaid_sanitizes_ids_and_direction() {
        let mut wide = run("dag-1", vec![node("1-a", "succeeded")]);
        wide.direction = "lr".into();
        let src = synthesize_mermaid(&wide);
        assert!(src.starts_with("graph LR\n"), "{src}");
        assert!(src.contains("1_a[\"✓ 1-a\"]"), "{src}");
    }

    #[test]
    fn run_diagram_renders_box_and_arrow_art() {
        let diagram = run_diagram(&chained_run(), 60, 20).expect("diagram must render");
        let text: String = diagram
            .iter()
            .flat_map(|line| line.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(text.contains('┌') && text.contains('┐'), "{text}");
        assert!(text.contains('▼'), "TD arrow glyph missing: {text}");
        assert!(text.contains("✓ 1-explore"), "{text}");
        assert!(text.contains("3-verify"), "{text}");
        // Every diagram row fits the band width.
        for line in &diagram {
            assert!(line_width(line) <= 60, "{line:?}");
        }
    }

    #[test]
    fn run_diagram_returns_none_without_dependency_edges() {
        let flat = run("dag-1", vec![node("a", "pending"), node("b", "pending")]);
        assert!(run_diagram(&flat, 60, 20).is_none());
        assert!(run_diagram(&run("dag-1", Vec::new()), 60, 20).is_none());
    }

    #[test]
    fn run_diagram_falls_back_when_too_tall() {
        let chained = chained_run();
        let full = run_diagram(&chained, 60, 100).expect("diagram renders");
        assert!(full.len() > MAX_NODE_ROWS);
        assert!(run_diagram(&chained, 60, full.len() as u16 - 1).is_none());
    }

    #[test]
    fn run_diagram_falls_back_when_too_wide() {
        let mut prev: Option<String> = None;
        let mut nodes = Vec::new();
        for i in 0..6 {
            let mut n = node(&format!("node-{i}"), "succeeded");
            if let Some(p) = &prev {
                n.depends_on = vec![p.clone()];
            }
            prev = Some(n.id.clone());
            nodes.push(n);
        }
        let mut wide = run("dag-1", nodes);
        wide.direction = "LR".into();
        // The horizontal chain exceeds a 20-column band.
        assert!(
            run_diagram(&wide, 20, 50).is_none(),
            "over-wide diagram must fall back"
        );
        // The same source lays out fine at a generous width.
        assert!(run_diagram(&wide, 200, 50).is_some());
    }

    #[test]
    fn band_rows_counts_diagram_height() {
        let chained = chained_run();
        let diagram = run_diagram(&chained, 80 - NODE_INDENT, u16::MAX).unwrap();
        assert_eq!(band_rows(&[chained], 80), 1 + diagram.len() as u16);
        // Flat runs (no edges) keep the text-row accounting.
        let flat = run("dag-1", vec![node("a", "pending"), node("b", "pending")]);
        assert_eq!(band_rows(&[flat], 80), 2);
    }

    #[test]
    fn render_dag_band_draws_box_diagram_that_fits() {
        let chained = chained_run();
        let rows = band_rows(std::slice::from_ref(&chained), 80);
        let area = Rect::new(0, 0, 80, rows);
        let mut buf = Buffer::empty(area);
        render_dag_band(&mut buf, area, &[chained], &HashMap::new(), 0);
        let text = buffer_text(&buf);
        assert!(
            text.lines().next().unwrap().contains("dag-1 · demo"),
            "{text}"
        );
        assert!(text.contains('┌') && text.contains('┐'), "{text}");
        assert!(text.contains('▼'), "TD arrow glyph missing: {text}");
        assert!(text.contains("✓ 1-explore"), "{text}");
        // The diagram replaces the wrapped text rows.
        assert!(!text.contains("✓ 1-explore · "), "{text}");
    }

    #[test]
    fn render_dag_band_falls_back_to_text_rows_when_too_tall() {
        let chained = chained_run();
        // Header + 3 text rows fit; the chained diagram does not.
        let area = Rect::new(0, 0, 80, 4);
        let mut buf = Buffer::empty(area);
        render_dag_band(&mut buf, area, &[chained], &HashMap::new(), 0);
        let text = buffer_text(&buf);
        assert!(text.contains("dag-1 · demo"), "{text}");
        assert!(text.contains("✓ 1-explore"), "{text}");
        assert!(text.contains("▶ 2-impl"), "{text}");
        assert!(!text.contains('┌'), "{text}");
    }

    // ── c/s meter accounting ────────────────────────────────────────────

    #[test]
    fn record_meters_accounts_output_token_deltas() {
        let t0 = Instant::now() + Duration::from_secs(60);
        let mut meters: HashMap<String, CpsMeter> = HashMap::new();
        let mut fixture = run("dag-1", vec![node("a", "running"), node("b", "running")]);
        fixture.nodes[0].output_tokens = Some(100);
        fixture.nodes[1].output_tokens = Some(200);
        record_meters_at(&mut meters, &[fixture.clone()], t0);
        // +1000 output tokens across both nodes half a second later.
        fixture.nodes[0].output_tokens = Some(600);
        fixture.nodes[1].output_tokens = Some(700);
        record_meters_at(
            &mut meters,
            &[fixture.clone()],
            t0 + Duration::from_millis(500),
        );
        let cps = meters["dag-1"].cps_at(t0 + Duration::from_millis(500));
        assert!((cps - 2000.0).abs() < 1e-6, "run c/s: {cps}");
        // Runs gone from the snapshot drop their meter.
        record_meters_at(&mut meters, &[], t0 + Duration::from_secs(1));
        assert!(meters.is_empty());
    }

    // ── full-band render ────────────────────────────────────────────────

    fn band_fixture() -> Vec<WireDagRunSnapshot> {
        let mut failed = node("2-impl", "failed");
        failed.error = Some("compile error".into());
        vec![
            run(
                "dag-1",
                vec![
                    node("1-explore", "succeeded"),
                    failed,
                    node("3-verify", "running"),
                    node("4-ship", "pending"),
                    node("5-done", "skipped"),
                    node("6-stop", "cancelled"),
                    node("7-wait", "ready"),
                ],
            ),
            run("dag-2", Vec::new()),
            run("dag-3", Vec::new()),
        ]
    }

    #[test]
    fn render_dag_band_header_nodes_and_more() {
        let dags = band_fixture();
        let area = Rect::new(0, 0, 100, 9);
        let mut buf = Buffer::empty(area);
        render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0);
        let text = buffer_text(&buf);
        let lines: Vec<&str> = text.lines().collect();
        // Header with spinner (a node is running), progress, and c/s.
        assert!(
            lines[0].contains("dag-1 · demo · 2/7 · c/s 0"),
            "{}",
            lines[0]
        );
        assert!(
            lines[0]
                .chars()
                .any(|c| ('\u{2800}'..='\u{28FF}').contains(&c)),
            "{}",
            lines[0]
        );
        // All seven state glyphs on the node row with the error summary.
        assert!(lines[1].contains("✓ 1-explore"), "{}", lines[1]);
        assert!(lines[1].contains("✗ 2-impl compile error"), "{}", lines[1]);
        assert!(lines[1].contains("▶ 3-verify"), "{}", lines[1]);
        assert!(lines[1].contains("· 4-ship"), "{}", lines[1]);
        assert!(lines[1].contains("↷ 5-done"), "{}", lines[1]);
        assert!(lines[1].contains("× 6-stop"), "{}", lines[1]);
        assert!(lines[1].contains("▸ 7-wait"), "{}", lines[1]);
        // Second run header, then the overflow line; the third run's id
        // never renders.
        assert!(
            lines[2].contains("dag-2 · demo · 0/0 · c/s 0"),
            "{}",
            lines[2]
        );
        assert!(lines[3].contains("… 1 more"), "{}", lines[3]);
        assert!(!text.contains("dag-3"), "{text}");
    }

    #[test]
    fn render_dag_band_node_colors() {
        let dags = band_fixture();
        let area = Rect::new(0, 0, 100, 9);
        let mut buf = Buffer::empty(area);
        render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0);
        let (x, y) = find_cell(&buf, "✓");
        assert_eq!(buf[(x, y)].fg, Color::Green);
        let (x, y) = find_cell(&buf, "✗");
        assert_eq!(buf[(x, y)].fg, Color::Red);
        let (x, y) = find_cell(&buf, "▶");
        assert_eq!(buf[(x, y)].fg, Color::Cyan);
        let (x, y) = find_cell(&buf, "×");
        assert_eq!(buf[(x, y)].fg, Color::DarkGray);
        assert!(buf[(x, y)].modifier.contains(Modifier::CROSSED_OUT));
        let (x, y) = find_cell(&buf, "↷");
        assert_eq!(buf[(x, y)].fg, Color::Gray);
        let (x, y) = find_cell(&buf, "▸");
        assert_eq!(buf[(x, y)].fg, Color::Yellow);
        // Pending: the `·` glyph also serves as the separator, so locate it
        // via its node id ("· 4-ship").
        let row_y = (0..area.height)
            .find(|&y| buffer_row(&buf, y).contains("4-ship"))
            .unwrap();
        let x = (2..area.width)
            .find(|&x| {
                buf[(x, row_y)].symbol() == "4"
                    && buf[(x - 1, row_y)].symbol() == " "
                    && buf[(x - 2, row_y)].symbol() == "·"
            })
            .unwrap();
        assert_eq!(buf[(x - 2, row_y)].fg, Color::DarkGray);
    }
}
