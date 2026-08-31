use super::*;
use std::time::Duration;

fn band() -> crate::ui::theme::DagBandStyle {
    crate::ui::theme::DagBandStyle::default()
}

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
        node_style("pending", &band()),
        ('·', Color::DarkGray, Modifier::empty())
    );
    assert_eq!(
        node_style("ready", &band()),
        ('▸', Color::Yellow, Modifier::empty())
    );
    assert_eq!(
        node_style("running", &band()),
        ('▶', Color::Cyan, Modifier::empty())
    );
    assert_eq!(
        node_style("succeeded", &band()),
        ('✓', Color::Green, Modifier::empty())
    );
    assert_eq!(
        node_style("failed", &band()),
        ('✗', Color::Red, Modifier::empty())
    );
    assert_eq!(
        node_style("cancelled", &band()),
        ('×', Color::DarkGray, Modifier::CROSSED_OUT)
    );
    assert_eq!(
        node_style("skipped", &band()),
        ('↷', Color::Gray, Modifier::empty())
    );
    // Unknown statuses fall back to pending.
    assert_eq!(node_style("bogus", &band()), node_style("pending", &band()));
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
    let line = run_header_line(&fixture, 0.0, 0, 30, &band());
    let text = line_text(&line);
    assert!(UnicodeWidthStr::width(text.as_str()) <= 30, "{text}");
    assert!(text.starts_with("dag-2 · "), "{text}");
    assert!(text.ends_with(" · 0/1 · c/s 0"), "{text}");
    assert!(text.contains('…'), "{text}");
    // Too narrow for any name: the name drops, header stays intact
    // (id 5 + separator 3 + tail 14 = 22 fixed cells).
    let narrow = run_header_line(&fixture, 0.0, 0, 22, &band());
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
    let line = run_header_line(&fixture, 84.0, 0, 80, &band());
    // Progress counts succeeded + skipped; c/s rounds.
    assert_eq!(line_text(&line), "dag-2 · demo · 2/6 · c/s 84");
}

#[test]
fn header_mini_spinner_when_running() {
    let fixture = run("dag-2", vec![node("a", "running")]);
    let line = run_header_line(&fixture, 0.0, 0, 80, &band());
    let first = line.spans[0].content.as_ref();
    let ch = first.chars().next().unwrap();
    assert!(('\u{2800}'..='\u{28FF}').contains(&ch), "{first:?}");
    // No spinner without a running node.
    let idle = run("dag-2", vec![node("a", "pending")]);
    let line = run_header_line(&idle, 0.0, 0, 80, &band());
    assert!(line_text(&line).starts_with("dag-2"));
}

#[test]
fn mini_spinner_speed_follows_cps() {
    // tick 1 = 10 ms: idle (130 ms/step) stays at step 0; fast
    // streaming (10 ms/step) advances immediately.
    assert_eq!(mini_spinner(0, 0.0).content, mini_spinner(1, 0.0).content);
    assert_ne!(
        mini_spinner(1, 0.0).content,
        mini_spinner(1, 10_000.0).content
    );
}

// ── node text rows ──────────────────────────────────────────────────

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
fn node_line_annotates_deps_and_errors() {
    let mut n = node("2-impl", "failed");
    n.depends_on = vec!["1-explore".into(), "side".into()];
    n.error = Some("compile error".into());
    let line = node_line(&n, &band(), 80);
    assert_eq!(line_text(&line), "✗ 2-impl ← 1-explore, side compile error");
    // No annotation without deps or error.
    let plain = node_line(&node("a", "succeeded"), &band(), 80);
    assert_eq!(line_text(&plain), "✓ a");
}

#[test]
fn node_line_fits_to_max_width() {
    let mut n = node("very-long-node-id", "succeeded");
    n.depends_on = vec!["another-long-dep".into()];
    let line = node_line(&n, &band(), 12);
    let text = line_text(&line);
    assert!(UnicodeWidthStr::width(text.as_str()) <= 12, "{text}");
    assert!(text.starts_with("✓ very"), "{text}");
    assert!(text.ends_with('…'), "{text}");
    // Generous width keeps the whole annotation.
    let wide = node_line(&n, &band(), 80);
    assert_eq!(line_text(&wide), "✓ very-long-node-id ← another-long-dep");
}

#[test]
fn run_node_lines_caps_at_max_rows() {
    let nodes: Vec<_> = (0..7)
        .map(|i| node(&format!("n{i}"), "succeeded"))
        .collect();
    let fixture = run("dag-1", nodes);
    let lines = run_node_lines(&fixture, &band(), 80);
    assert_eq!(lines.len(), MAX_NODE_ROWS + 1); // 3 nodes + `… 4 more`
    assert_eq!(line_text(&lines[0]), "✓ n0");
    assert!(line_text(&lines[MAX_NODE_ROWS]).contains("… 4 more"));
    // Under the cap: no tail row.
    let small = run("dag-1", vec![node("a", "succeeded")]);
    assert_eq!(run_node_lines(&small, &band(), 80).len(), 1);
}

// ── band layout: bordered boxes, side by side when they fit ─────────

#[test]
fn band_rows_counts_boxes_and_more_line() {
    assert_eq!(band_rows(&[], 80), 0);
    // One node → top border + node row + bottom border.
    let one = vec![run("dag-1", vec![node("a", "pending")])];
    assert_eq!(band_rows(&one, 80), 3);
    // Three empty runs: two header-only boxes side by side (2 rows) + `… 1 more`.
    let three = vec![
        run("dag-1", Vec::new()),
        run("dag-2", Vec::new()),
        run("dag-3", Vec::new()),
    ];
    assert_eq!(band_rows(&three, 80), 3);
    // Per-run cap: top border + 3 node rows + `… N more` + bottom border.
    let ids: Vec<String> = (0..50).map(|i| format!("n{i}")).collect();
    let nodes: Vec<_> = ids.iter().map(|id| node(id, "pending")).collect();
    let big = vec![run("dag-1", nodes)];
    assert_eq!(band_rows(&big, 20), MAX_NODE_ROWS as u16 + 3);
}

#[test]
fn band_rows_side_by_side_when_width_fits() {
    // Two chained runs: each box = top + 3 node rows + bottom = 5 rows.
    let dags = vec![chained_run(), chained_run()];
    // Wide band: boxes sit next to each other, height = taller box = 5.
    assert_eq!(band_rows(&dags, 80), 5);
    // Narrow band: boxes stack, heights sum (5 + 5 = 10).
    let ids = [
        "node-aaaaaaaaaaaa",
        "node-bbbbbbbbbbbb",
        "node-cccccccccccc",
    ];
    let wide_nodes: Vec<_> = ids.iter().map(|id| node(id, "succeeded")).collect();
    let wide_run = run("dag-1", wide_nodes);
    let dags = vec![wide_run.clone(), wide_run.clone()];
    assert_eq!(band_rows(&dags, 20), 10);
    // A single run never stacks with itself.
    assert_eq!(band_rows(std::slice::from_ref(&wide_run), 20), 5);
}

#[test]
fn render_dag_band_draws_bordered_boxes_side_by_side() {
    // A 3-node chained run next to an empty run: the empty box stretches to
    // the taller height so both bottoms align flush on the same row.
    let dags = vec![chained_run(), run("dag-2", Vec::new())];
    let rows = band_rows(&dags, 80);
    assert_eq!(rows, 5);
    let area = Rect::new(0, 0, 80, rows);
    let mut buf = Buffer::empty(area);
    render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0, &band());
    let text = buffer_text(&buf);
    let lines: Vec<&str> = text.lines().collect();
    // Both box tops on the same row (side by side). The first run has a
    // running node, so its header starts with the mini spinner after the
    // `╭─ ` border prefix.
    assert!(lines[0].starts_with("╭─ "), "{}", lines[0]);
    assert!(lines[0].contains("dag-1 · demo"), "{}", lines[0]);
    assert!(lines[0].contains("╭─ dag-2 · demo"), "{}", lines[0]);
    // Node rows carry the dependency annotation.
    assert!(lines[1].contains("✓ 1-explore"), "{}", lines[1]);
    assert!(lines[2].contains("▶ 2-impl ← 1-explore"), "{}", lines[2]);
    assert!(lines[3].contains("· 3-verify ← 2-impl"), "{}", lines[3]);
    // Equal-height alignment: the empty box renders bordered empty rows, and
    // both bottom borders land on the same (last) row.
    assert!(lines[1].contains("│"), "{}", lines[1]);
    assert_eq!(
        lines[4].matches('╰').count(),
        2,
        "both boxes must bottom-align on the last row: {}",
        lines[4]
    );
    // Every row exactly fills its box width (no overflow past the border).
    for line in &lines {
        let trimmed = line.trim_end();
        assert!(
            UnicodeWidthStr::width(trimmed) <= 80,
            "row too wide: {trimmed}"
        );
    }
}

#[test]
fn render_dag_band_stacks_when_too_narrow() {
    let dags = vec![chained_run(), chained_run()];
    let rows = band_rows(&dags, 20);
    assert_eq!(rows, 10);
    let area = Rect::new(0, 0, 20, rows);
    let mut buf = Buffer::empty(area);
    render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0, &band());
    let text = buffer_text(&buf);
    let lines: Vec<&str> = text.lines().collect();
    // Box 1 occupies rows 0-4, box 2 rows 5-9.
    assert!(
        lines[0].starts_with("╭─ ") && lines[0].contains("dag-1 · demo"),
        "{}",
        lines[0]
    );
    assert!(
        lines[5].starts_with("╭─ ") && lines[5].contains("dag-1 · demo"),
        "{}",
        lines[5]
    );
    assert!(lines[4].contains("╰"), "{}", lines[4]);
    assert!(lines[9].contains("╰"), "{}", lines[9]);
}

#[test]
fn render_dag_band_fits_header_and_nodes_into_narrow_box() {
    // A very long run name must not overflow the box.
    let mut wide = run("dag-1", vec![node("a", "succeeded")]);
    wide.name = "this-is-an-extremely-long-run-name-that-cannot-possibly-fit".into();
    let area = Rect::new(0, 0, 24, 3);
    let mut buf = Buffer::empty(area);
    render_dag_band(&mut buf, area, &[wide], &HashMap::new(), 0, &band());
    let text = buffer_text(&buf);
    for row in text.lines() {
        assert!(
            UnicodeWidthStr::width(row.trim_end()) <= 24,
            "row overflows narrow band: {row}"
        );
    }
    assert!(text.contains('…'), "{text}");
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
    failed.depends_on = vec!["1-explore".into()];
    vec![
        run(
            "dag-1",
            vec![
                node("1-explore", "succeeded"),
                failed,
                node("3-verify", "running"),
            ],
        ),
        run("dag-2", Vec::new()),
        run("dag-3", Vec::new()),
    ]
}

#[test]
fn render_dag_band_header_nodes_and_more() {
    let dags = band_fixture();
    let area = Rect::new(0, 0, 100, 7);
    let mut buf = Buffer::empty(area);
    render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0, &band());
    let text = buffer_text(&buf);
    let lines: Vec<&str> = text.lines().collect();
    // Top border embeds the header (spinner: a node is running).
    assert!(lines[0].starts_with("╭─ "), "{}", lines[0]);
    assert!(
        lines[0].contains("dag-1 · demo · 1/3 · c/s 0"),
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
    // One node row per node, dependency annotation included.
    assert!(lines[1].contains("✓ 1-explore"), "{}", lines[1]);
    assert!(
        lines[2].contains("✗ 2-impl ← 1-explore compile error"),
        "{}",
        lines[2]
    );
    assert!(lines[3].contains("▶ 3-verify"), "{}", lines[3]);
    // Bottom border of the first box, second box top on the same row.
    assert!(lines[4].contains("╰"), "{}", lines[4]);
    // The third run folds into the `… 1 more` row after the boxes.
    assert!(text.contains("… 1 more"), "{text}");
    assert!(!text.contains("dag-3"), "{text}");
}

#[test]
fn render_dag_band_node_colors() {
    let dags = band_fixture();
    let area = Rect::new(0, 0, 100, 7);
    let mut buf = Buffer::empty(area);
    render_dag_band(&mut buf, area, &dags, &HashMap::new(), 0, &band());
    let (x, y) = find_cell(&buf, "✓");
    assert_eq!(buf[(x, y)].fg, Color::Green);
    let (x, y) = find_cell(&buf, "✗");
    assert_eq!(buf[(x, y)].fg, Color::Red);
    let (x, y) = find_cell(&buf, "▶");
    assert_eq!(buf[(x, y)].fg, Color::Cyan);
    // The dependency annotation is dimmed.
    let row_y = (0..area.height)
        .find(|&y| buffer_row(&buf, y).contains("2-impl"))
        .unwrap();
    let x = (0..area.width)
        .find(|&x| buf[(x, row_y)].symbol() == "←")
        .unwrap();
    assert!(buf[(x, row_y)].modifier.contains(Modifier::DIM));
    // The border uses the edge color.
    let (bx, by) = find_cell(&buf, "╭");
    assert_eq!(buf[(bx, by)].fg, band().edge);
}
