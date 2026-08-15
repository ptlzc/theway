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
