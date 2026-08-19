    use super::*;

    fn styles() -> MermaidStyles {
        let s = Style::default();
        MermaidStyles {
            border: s,
            node_text: s,
            edge: s,
            edge_label: s,
            title: s,
        }
    }

    fn plain(src: &str) -> String {
        render(src, &styles(), Some(120))
            .unwrap()
            .plain_lines
            .join("\n")
    }

    #[test]
    fn parses_nodes_edges_and_direction() {
        let g = parse_graph("flowchart LR\n  A[Start] --> B[End]").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.nodes[0].label, "Start");
        assert_eq!(g.nodes[1].label, "End");
        assert!(g.dir == Dir::Right);
    }

    #[test]
    fn non_flowchart_returns_none_from_parse() {
        assert!(parse_graph("sequenceDiagram\n  A->>B: hi").is_none());
    }

    #[test]
    fn html_tags_are_stripped_from_labels() {
        let g = parse_graph("flowchart TD\n  A[\"<b>Bold</b> and <i>italic</i>\"] --> B").unwrap();
        assert_eq!(g.nodes[0].label, "Bold and italic");
    }

    #[test]
    fn br_tag_becomes_a_space() {
        let g = parse_graph("flowchart TD\n  A[\"Line1<br/>Line2<br>Line3\"]").unwrap();
        assert_eq!(g.nodes[0].label, "Line1 Line2 Line3");
    }

    #[test]
    fn markdown_string_strips_bold_italic_and_code() {
        let g = parse_graph(
            "flowchart TD\n  A[\"`**Start** here`\"] --> B[\"`Save to **database**`\"]\n  B --> C[\"`**Done!**`\"]",
        )
        .unwrap();
        assert_eq!(g.nodes[0].label, "Start here");
        assert_eq!(g.nodes[1].label, "Save to database");
        assert_eq!(g.nodes[2].label, "Done!");
    }

    #[test]
    fn markdown_string_preserves_snake_case_and_strips_inline_code() {
        let g = parse_graph("flowchart TD\n  A[\"`_italic_ uses `vocab_size` with __all__`\"]")
            .unwrap();
        assert_eq!(g.nodes[0].label, "italic uses vocab_size with all");
    }

    #[test]
    fn markdown_string_edge_label_is_stripped() {
        let g =
            parse_graph("flowchart TD\n  A -->|\"`**yes**`\"| B\n  A -->|\"`__no__`\"| C").unwrap();
        assert_eq!(g.edges[0].label.as_deref(), Some("yes"));
        assert_eq!(g.edges[1].label.as_deref(), Some("no"));
    }

    #[test]
    fn plain_label_keeps_literal_text_and_underscores() {
        // Not a markdown string (no backtick wrapper): Mermaid renders it
        // literally, so brackets, snake_case, and any `*`/`_` must survive.
        let g = parse_graph("flowchart TD\n  A[\"[ 464, 3797 ] seq_len d_model\"]").unwrap();
        assert_eq!(g.nodes[0].label, "[ 464, 3797 ] seq_len d_model");
    }

    #[test]
    fn code_and_span_tags_are_stripped() {
        let g = parse_graph(
            "flowchart TD\n  A[\"<code>vocab_size</code> <span style=\\\"color:red\\\">x</span>\"]",
        )
        .unwrap();
        assert_eq!(g.nodes[0].label, "vocab_size x");
    }

    #[test]
    fn bare_angle_brackets_are_kept() {
        let g = parse_graph("flowchart TD\n  A[\"a < b and c > d\"]").unwrap();
        assert_eq!(g.nodes[0].label, "a < b and c > d");
    }

    #[test]
    fn generic_types_are_not_stripped_as_html() {
        // `<String>` / `<i32>` / `<id>` look like tags but are not HTML
        // formatting tags, so they must survive (only b/i/code/span/… etc. and
        // <br> are stripped).
        let g = parse_graph(
            "flowchart TD\n  A[\"Returns Vec<String>\"] --> B[\"Option<i32> for <id>\"]",
        )
        .unwrap();
        assert_eq!(g.nodes[0].label, "Returns Vec<String>");
        assert_eq!(g.nodes[1].label, "Option<i32> for <id>");
    }

    #[test]
    fn decode_html_entities_covers_named_numeric_and_double_escape() {
        assert_eq!(
            decode_html_entities("&lt;a&gt; &amp; &quot;x&quot; &apos;y&apos;"),
            "<a> & \"x\" 'y'"
        );
        assert_eq!(decode_html_entities("it&#39;s &#60;ok&#62;"), "it's <ok>");
        assert_eq!(
            decode_html_entities("&#x3c;tag&#X3E; &#x27;q&#x27;"),
            "<tag> 'q'"
        );
        // `&amp;lt;` must yield the literal `&lt;`, never `<`.
        assert_eq!(decode_html_entities("&amp;lt;"), "&lt;");
        assert_eq!(decode_html_entities("a &foo; b & c"), "a &foo; b & c");
        // Control chars (NUL collides with CONT, ESC injects ANSI) never decode.
        assert_eq!(decode_html_entities("a&#27;b&#0;c"), "a&#27;b&#0;c");
        assert_eq!(decode_html_entities("x&#x1b;y"), "x&#x1b;y");
    }

    #[test]
    fn entity_escaped_flowchart_label_decodes_in_box_art() {
        let src = "flowchart LR\n  YAML[\"models-config/&lt;model&gt;/&lt;env&gt;.yaml\\nenterprise_api_config:\"]\n  PY[\"model_config_map.py\\nlanguage_model_dict_to_proto()\"]\n  YAML --> PY";
        let g = parse_graph(src).unwrap();
        assert!(
            g.nodes[0]
                .label
                .contains("models-config/<model>/<env>.yaml"),
            "{}",
            g.nodes[0].label
        );
        let art = plain(src);
        assert!(art.contains("<model>") && art.contains("<env>"), "{art}");
        assert!(!art.contains("&lt;") && !art.contains("&gt;"), "{art}");
    }

    #[test]
    fn direct_push_sinks_decode_entities() {
        // Entities contain `;`, which split_statements treats as a separator, so
        // they reach a sink intact only inside quotes; assert through the real
        // parsers where such quoting works.
        let g = parse_state(
            "stateDiagram-v2\n  state \"work &lt;job&gt;\" as J\n  Idle --> Run: \"on &lt;go&gt;\"\n  Run: \"d &lt;e&gt;\"",
        )
        .unwrap();
        let node = |s: &str| g.nodes.iter().any(|n| n.label.contains(s));
        let edge = |s: &str| {
            g.edges
                .iter()
                .any(|e| e.label.as_deref().is_some_and(|l| l.contains(s)))
        };
        assert!(node("work <job>") && node("d <e>") && edge("on <go>"));
        assert!(!node("&lt;") && !edge("&lt;"));

        let (cg, _) = parse_class("classDiagram\n  A --> B : \"uses &lt;X&gt;\"").unwrap();
        assert!(cg.edges.iter().any(|e| {
            e.label
                .as_deref()
                .is_some_and(|l| l.contains("uses <X>") && !l.contains("&lt;"))
        }));

        let s = parse_sequence(
            "sequenceDiagram\n  A->>B: \"call &lt;svc&gt;\"\n  Note over A,B: \"memo &lt;o&gt;\"\n  alt \"c &lt;x&gt;\"\n    A->>B: ok\n  end",
        )
        .unwrap();
        assert!(s.items.iter().any(|it| matches!(it,
            SeqItem::Message { text: Some(t), .. } if t.contains("call <svc>") && !t.contains("&lt;"))));
        assert!(s.items.iter().any(|it| matches!(it,
            SeqItem::Note { text, .. } if text.contains("memo <o>") && !text.contains("&lt;"))));
        assert!(s.items.iter().any(|it| matches!(it,
            SeqItem::Divider { text } if text.contains("c <x>") && !text.contains("&lt;"))));

        // Class members and ER attributes have no clean quoted form (splitter
        // fragments unquoted `;`; ER drops quoted text as a comment), so exercise
        // those decodes at the finalizer directly.
        let mut member = ClassInfo::default();
        push_member(&mut member, "+run &lt;R&gt;");
        assert_eq!(member.attrs, vec!["+run <R>".to_string()]);
        let mut attr = ClassInfo::default();
        push_er_attribute(&mut attr, "string &lt;pk&gt;");
        assert_eq!(attr.attrs, vec!["string <pk>".to_string()]);
    }

    #[test]
    fn quoted_label_with_inner_brackets_is_one_node() {
        let g = parse_graph(
            "flowchart TD\n  IDs[\"<b>Token IDs</b><br/>[ 464, 3797 ]<br/><i>indices</i>\"]",
        )
        .unwrap();
        assert_eq!(g.nodes.len(), 1, "inner brackets must not split the node");
        assert_eq!(g.edges.len(), 0, "no phantom edges from <br/> + brackets");
        assert_eq!(g.nodes[0].label, "Token IDs [ 464, 3797 ] indices");
    }

    #[test]
    fn unquoted_label_with_embedded_quote_closes_at_bracket() {
        let g = parse_graph("flowchart TD\n  A[5\" pipe] --> B[24\" display]").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.nodes[0].label, "5\" pipe");
        assert_eq!(g.nodes[1].label, "24\" display");
    }

    #[test]
    fn quoted_label_with_inner_parens_is_one_node() {
        let g =
            parse_graph("flowchart TD\n  A[\"Tokenizer (BPE / WordPiece)\"] --> B[Done]").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.nodes[0].label, "Tokenizer (BPE / WordPiece)");
    }

    #[test]
    fn diagram_with_html_labels_renders_without_tag_artifacts() {
        let src = "flowchart TD\n  IDs[\"<b>3. Token IDs</b><br/>[ 464, 3797 ]<br/><i>indices</i>\"] --> Out[\"<b>done</b>\"]";
        let out = plain(src);
        assert!(!out.contains("<b>"), "raw HTML tag leaked:\n{out}");
        assert!(!out.contains("</"), "raw closing tag leaked:\n{out}");
        assert!(!out.contains("br/"), "phantom br artifact leaked:\n{out}");
        assert!(out.contains("Token IDs"), "label text missing:\n{out}");
    }

    #[test]
    fn ranks_ignore_back_edges() {
        let g = parse_graph("graph TD\n A-->B\n B-->C\n C-->A").unwrap();
        let r = compute_ranks(&g);
        let idx = |id: &str| g.index[id];
        assert_eq!(r[idx("A")], 0);
        assert_eq!(r[idx("B")], 1);
        assert_eq!(r[idx("C")], 2);
    }

    #[test]
    fn td_render_has_boxes_labels_and_arrow() {
        let out = plain("graph TD\n A[Start] --> B[End]");
        assert!(out.contains("Start"), "{out}");
        assert!(out.contains("End"), "{out}");
        assert!(out.contains('┌') || out.contains('╭'), "{out}");
        assert!(out.contains('▼'), "{out}");
    }

    #[test]
    fn edge_label_is_rendered() {
        let out = plain("graph TD\n A-->|yes| B");
        assert!(out.contains("yes"), "{out}");
    }

    #[test]
    fn lr_is_shorter_than_td_for_a_chain() {
        let chain = "A --> B --> C --> D";
        let td = render(&format!("graph TD\n {chain}"), &styles(), Some(120))
            .unwrap()
            .plain_lines
            .len();
        let lr = render(&format!("flowchart LR\n {chain}"), &styles(), Some(120))
            .unwrap()
            .plain_lines
            .len();
        assert!(lr < td, "expected LR ({lr}) shorter than TD ({td})");
    }

    #[test]
    fn unsupported_diagram_uses_fallback_box() {
        let out = plain("gantt\n title Plan\n section A\n task :a1, 2024-01-01, 30d");
        assert!(out.contains("mermaid: gantt"), "{out}");
        assert!(out.contains("Plan"), "{out}");
    }

    #[test]
    fn blank_source_returns_none() {
        assert!(render("   \n  ", &styles(), Some(80)).is_none());
    }

    #[test]
    fn inline_label_with_x_or_o_letters() {
        let g = parse_graph("graph TD\n A -- no exit --> B").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].label.as_deref(), Some("no exit"));
    }

    #[test]
    fn wide_glyph_box_stays_aligned() {
        let lines = render("graph TD\n A[日本語ab]", &styles(), Some(120))
            .unwrap()
            .plain_lines;
        let widths: Vec<usize> = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.width())
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "box rows must share one width: {widths:?}\n{lines:?}"
        );
        assert!(!lines.iter().any(|l| l.contains(CONT)), "sentinel leaked");
    }

    #[test]
    fn merge_has_single_arrowhead() {
        let out = plain("graph TD\n A[aaa] --> D[ddddddd]\n B[bb] --> D\n C[ccccc] --> D");
        let arrows = out.chars().filter(|&c| c == '▼').count();
        assert_eq!(arrows, 1, "merge edges share one arrowhead:\n{out}");
        assert!(!out.contains("▼▼"), "must not stack arrowheads:\n{out}");
    }

    #[test]
    fn long_label_wraps_without_truncation() {
        let out =
            plain("graph TD\n A[Check if the user has permission to access resource] --> B[Done]");
        assert!(out.contains("permission"), "{out}");
        assert!(out.contains("resource"), "{out}");
        assert!(!out.contains('…'), "should wrap, not truncate:\n{out}");
    }

    #[test]
    fn very_long_label_truncates_after_max_lines() {
        let long = "alpha ".repeat(40);
        let out = plain(&format!("graph TD\n A[{}] --> B[x]", long.trim()));
        assert!(out.contains('…'), "should truncate past max lines:\n{out}");
    }

    #[test]
    fn wrap_label_breaks_long_identifier_on_boundary() {
        let lines = wrap_label("mark_filter_restore_context", WRAP_WIDTH, MAX_LINES);
        // The first line ends on an identifier boundary, not a mid-segment slice.
        assert!(
            lines[0].ends_with('_'),
            "first line must end on a boundary: {lines:?}"
        );
        // Every break (all but the last line) lands on a boundary char.
        for line in &lines[..lines.len() - 1] {
            assert!(
                line.ends_with(LABEL_BREAK_CHARS),
                "line must break on a boundary: {line:?}"
            );
        }
        // Nothing is lost: the wrapped lines reconstruct the original word.
        assert_eq!(lines.concat(), "mark_filter_restore_context");
    }

    #[test]
    fn wrap_label_token_without_break_char_falls_back_per_char() {
        let token = "a".repeat(40);
        let lines = wrap_label(&token, WRAP_WIDTH, MAX_LINES);
        // No boundary char -> per-char hard break across multiple lines.
        assert!(lines.len() >= 2, "must hard-break: {lines:?}");
        // 40 narrow chars fit in <= MAX_LINES, so nothing is truncated or lost.
        assert_eq!(lines.concat(), token);
    }

    #[test]
    fn flowchart_long_identifier_breaks_on_boundary_not_mid_segment() {
        let out = plain("graph TD\n A[mark_filter_restore_context] --> B[Done]");
        // The boundary-respecting pieces are present in the rendered art; the
        // `wrap_label_breaks_long_identifier_on_boundary` unit test proves there
        // is no mid-segment slice (losslessly), so no offset-coupled guard here.
        assert!(out.contains("mark_filter_restore_"), "{out}");
        assert!(out.contains("context"), "{out}");
    }

    #[test]
    fn wrap_label_mixed_boundary_then_no_boundary_tail() {
        let token = String::from("ab_") + &"c".repeat(40);
        let lines = wrap_label(&token, WRAP_WIDTH, MAX_LINES);
        // The boundary is taken first ...
        assert!(
            lines[0].ends_with('_'),
            "first break on boundary: {lines:?}"
        );
        // ... then the long no-boundary tail falls back to a per-char break.
        assert!(
            lines[1..].iter().any(|l| !l.contains(LABEL_BREAK_CHARS)),
            "a later line must be a per-char break: {lines:?}"
        );
        // 43 cols < MAX_LINES*WRAP_WIDTH, so it must not truncate; fully lossless.
        assert_eq!(lines.concat(), token);
    }

    #[test]
    fn wrap_label_boundary_breaking_still_truncates_at_max_lines() {
        let id = ["segment"; 20].join("_");
        let lines = wrap_label(&id, WRAP_WIDTH, MAX_LINES);
        // The identifier far exceeds MAX_LINES*WRAP_WIDTH, so it truncates ...
        assert_eq!(lines.len(), MAX_LINES);
        // ... with the ellipsis still on the final line.
        assert!(
            lines.last().unwrap().ends_with('…'),
            "truncation must keep the ellipsis: {lines:?}"
        );
    }

    #[test]
    fn bt_flips_orientation() {
        let out = plain("flowchart BT\n A[first] --> B[second] --> C[third]");
        let lines: Vec<&str> = out.lines().collect();
        let row = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            row("third") < row("first"),
            "BT: 'third' should sit above 'first':\n{out}"
        );
    }

    #[test]
    fn rl_flips_orientation() {
        let out = plain("flowchart RL\n A[first] --> B[second] --> C[third]");
        let line = out.lines().find(|l| l.contains("first")).unwrap();
        assert!(
            line.find("third") < line.find("first"),
            "RL: 'third' should sit left of 'first':\n{out}"
        );
    }

    #[test]
    fn undirected_piped_label_has_no_arrowhead() {
        let out = plain("graph TD\n A ---|maybe| B");
        assert!(out.contains("maybe"), "{out}");
        assert!(
            !out.contains('▼'),
            "undirected link should not draw an arrow:\n{out}"
        );
    }

    #[test]
    fn chain_edges_are_straight() {
        let out = plain("graph TD\n A[aaaa] --> B[b] --> C[cccccccc]");
        for line in out.lines() {
            assert!(
                !line.contains('└') || !line.contains('┐'),
                "chain should not jog: {line:?}"
            );
        }
    }

    #[test]
    fn adversarial_chain_falls_back() {
        let mut src = String::from("graph TD\n");
        for i in 0..10_000 {
            src.push_str(&format!(" N{i} --> N{}\n", i + 1));
        }
        let out = plain(&src);
        assert!(out.contains("mermaid: graph"), "expected fallback:\n{out}");
    }

    #[test]
    fn single_statement_chain_over_cap_falls_back() {
        let mut src = String::from("graph LR\n ");
        for i in 0..10_000 {
            src.push_str(&format!("N{i}-->"));
        }
        src.push_str("N10000");
        let out = plain(&src);
        assert!(out.contains("mermaid: graph"), "expected fallback");
    }

    #[test]
    fn deep_chain_within_caps_renders() {
        let mut src = String::from("graph TD\n");
        for i in 0..100 {
            src.push_str(&format!(" N{i} --> N{}\n", i + 1));
        }
        let out = render(&src, &styles(), Some(200)).unwrap().plain_lines;
        let joined = out.join("\n");
        assert!(joined.contains("N0"), "{joined}");
        assert!(joined.contains("N100"), "{joined}");
        assert!(joined.contains('▼'), "{joined}");
    }

    #[test]
    fn fallback_styled_and_plain_widths_match() {
        let art = render("gantt\n title Plan\n a\n", &styles(), Some(120)).unwrap();
        assert_eq!(art.styled_lines.len(), art.plain_lines.len());
        let frame_w = art.plain_lines[0].width();
        for (styled, plain) in art.styled_lines.iter().zip(&art.plain_lines) {
            let styled_w: usize = styled
                .spans
                .iter()
                .map(|s| s.content.as_ref().width())
                .sum();
            assert_eq!(styled_w, plain.width(), "styled/plain widths diverge");
            assert_eq!(plain.width(), frame_w, "fallback box must be rectangular");
        }
    }

    #[test]
    fn over_wide_diagram_falls_back() {
        let src = "flowchart LR\n A[aaaaaaaaaaaaaaaaaaaa] --> B[bbbbbbbbbbbbbbbbbbbb] --> C[cccccccccccccccccccc]";
        let out = render(src, &styles(), Some(40)).unwrap().plain_lines;
        let joined = out.join("\n");
        assert!(
            joined.contains("mermaid: flowchart"),
            "expected fallback for over-wide diagram:\n{joined}"
        );
        let max_w = out.iter().map(|l| l.width()).max().unwrap_or(0);
        let fits = render(src, &styles(), Some(120)).unwrap().plain_lines;
        assert!(
            fits.iter().any(|l| l.contains('▶')),
            "same diagram should render when it fits"
        );
        assert!(max_w <= src.len(), "fallback width bounded by source");
    }

    #[test]
    fn too_wide_fallback_appends_hint_below_box() {
        let src = "flowchart LR\n A[aaaaaaaaaaaaaaaaaaaa] --> B[bbbbbbbbbbbbbbbbbbbb] --> C[cccccccccccccccccccc]";
        let out = render(src, &styles(), Some(40)).unwrap().plain_lines;
        let joined = out.join("\n");

        assert!(
            joined.contains("mermaid: flowchart"),
            "plain header:\n{joined}"
        );
        assert!(
            !joined.contains("(too wide)"),
            "header stays plain:\n{joined}"
        );
        assert!(
            joined.contains("flowchart LR"),
            "raw source kept:\n{joined}"
        );

        let bottom = out
            .iter()
            .position(|l| l.contains('╰'))
            .expect("box bottom");
        let note = out
            .iter()
            .position(|l| l.contains("too wide"))
            .expect("note row");
        assert!(note > bottom, "note must be below the box:\n{joined}");
        assert!(
            joined.contains("open the image"),
            "note points at the image:\n{joined}"
        );

        assert!(
            out.iter().all(|l| l.width() <= 40),
            "fits 40 cols:\n{joined}"
        );
    }

    #[test]
    fn unsupported_diagram_fallback_not_flagged_too_wide() {
        let out = plain("gantt\n title Plan\n section A\n task :a1, 2024-01-01, 30d");
        assert!(out.contains("mermaid: gantt"), "{out}");
        assert!(
            !out.contains("too wide"),
            "unsupported type is not a width problem:\n{out}"
        );
    }

    #[test]
    fn fitting_diagram_has_no_width_warning() {
        let out = plain("flowchart LR\n A[Start] --> B[End]");
        assert!(
            !out.contains("too wide"),
            "fitting diagram must not warn:\n{out}"
        );
        assert!(
            !out.contains("mermaid: flowchart"),
            "should draw art, not box:\n{out}"
        );
        assert!(out.contains('▶'), "should draw edges:\n{out}");
    }

    #[test]
    fn bidirectional_link_draws_both_arrowheads() {
        let lr = plain("flowchart LR\n A <--> B");
        assert!(lr.contains('◄') && lr.contains('▶'), "{lr}");
        let td = plain("graph TD\n A <--> B");
        assert!(td.contains('▲') && td.contains('▼'), "{td}");
    }

    #[test]
    fn reversed_arrow_swaps_edge_direction() {
        let g = parse_graph("graph TD\n A <-- B").unwrap();
        let idx = |id: &str| g.index[id];
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].from, idx("B"));
        assert_eq!(g.edges[0].to, idx("A"));
        assert_eq!(g.edges[0].head_to, Head::Arrow);
        assert_eq!(g.edges[0].head_from, Head::None);
        let out = plain("graph TD\n A <-- B");
        let lines: Vec<&str> = out.lines().collect();
        let row = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(row("B") < row("A"), "B should rank above A:\n{out}");
    }

    #[test]
    fn semicolon_and_comment_survive_inside_quoted_label() {
        let g = parse_graph("graph TD\n A[\"wait; 50%% done\"] --> B").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.nodes[0].label, "wait; 50%% done");
    }

    #[test]
    fn comment_outside_quotes_is_stripped() {
        let g =
            parse_graph("graph TD %% main flow\n A --> B %% trailing\n %% full line\n").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges.len(), 1);
    }

    #[test]
    fn skip_edge_routes_around_intermediate_boxes() {
        let out = plain("graph TD\n A --> B\n B --> C\n A --> C");
        assert!(!out.contains('┼'), "no border corruption:\n{out}");
        assert!(
            out.contains('◄'),
            "skip edge enters target from lane:\n{out}"
        );
    }

    fn ordered_ranks(src: &str) -> (Graph, Vec<usize>, Vec<Vec<usize>>) {
        let g = parse_graph(src).unwrap();
        let ranks = compute_ranks(&g);
        let max_rank = *ranks.iter().max().unwrap();
        let mut by_rank: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
        for (idx, &r) in ranks.iter().enumerate() {
            by_rank[r].push(idx);
        }
        order_ranks(&mut by_rank, &g.edges, &ranks);
        (g, ranks, by_rank)
    }

    #[test]
    fn order_ranks_removes_avoidable_crossing() {
        let (g, ranks, by_rank) = ordered_ranks("graph TD\n C[ccc]\n D[ddd]\n A --> D\n B --> C");
        let mut pos = vec![0usize; g.nodes.len()];
        for row in &by_rank {
            for (i, &v) in row.iter().enumerate() {
                pos[v] = i;
            }
        }
        assert_eq!(count_crossings(&g.edges, &ranks, &pos), 0);
        let idx = |id: &str| g.index[id];
        assert!(pos[idx("D")] < pos[idx("C")], "D follows parent A leftward");
    }

    #[test]
    fn order_ranks_keeps_crossing_free_order() {
        let (g, ranks, by_rank) = ordered_ranks("graph TD\n A --> C\n B --> D");
        let idx = |id: &str| g.index[id];
        assert_eq!(by_rank[0], vec![idx("A"), idx("B")]);
        assert_eq!(by_rank[1], vec![idx("C"), idx("D")]);
        let mut pos = vec![0usize; g.nodes.len()];
        for row in &by_rank {
            for (i, &v) in row.iter().enumerate() {
                pos[v] = i;
            }
        }
        assert_eq!(count_crossings(&g.edges, &ranks, &pos), 0);
    }

    #[test]
    fn crossing_edges_render_untangled() {
        let out = plain("graph TD\n C[ccc]\n D[ddd]\n A --> D\n B --> C");
        let row = out
            .lines()
            .find(|l| l.contains("ccc") && l.contains("ddd"))
            .unwrap();
        assert!(
            row.find("ddd") < row.find("ccc"),
            "children reorder under their parents:\n{out}"
        );
        assert!(!out.contains('┼'), "{out}");
    }

    #[test]
    fn three_layer_weave_untangles() {
        let (g, ranks, by_rank) = ordered_ranks(
            "graph TD\n X[x]\n Y[y]\n A --> Y\n B --> X\n X --> Q\n Y --> P\n P[p]\n Q[q]",
        );
        let mut pos = vec![0usize; g.nodes.len()];
        for row in &by_rank {
            for (i, &v) in row.iter().enumerate() {
                pos[v] = i;
            }
        }
        assert_eq!(
            count_crossings(&g.edges, &ranks, &pos),
            0,
            "both layers untangle"
        );
    }

    #[test]
    fn unavoidable_crossing_gets_separate_bus_rows() {
        let crossing = plain("graph TD\n A --> D[ddd]\n A --> C[ccc]\n B --> C\n B --> D");
        let parallel = plain("graph TD\n A --> C[ccc]\n B --> D[ddd]");
        assert!(crossing.contains('┼'), "wire crossing renders:\n{crossing}");
        assert_eq!(
            crossing.lines().count(),
            parallel.lines().count() + 1,
            "crossing pair claims one extra bus row:\n{crossing}"
        );
        assert_eq!(
            crossing.chars().filter(|&c| c == '▼').count(),
            2,
            "{crossing}"
        );
    }

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mermaid/unit/diagrams.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mermaid/unit/fallback.rs"));
