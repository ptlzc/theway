    #[test]
    fn sequence_declared_order_wins() {
        let out = plain("sequenceDiagram\n participant B\n participant A\n A->>B: hi");
        let line = out.lines().nth(1).unwrap();
        assert!(
            line.find('B') < line.find('A'),
            "B declared first sits left:\n{out}"
        );
    }

    #[test]
    fn sequence_self_message_loops() {
        let out = plain("sequenceDiagram\n A->>A: think");
        assert!(out.contains('╮'), "{out}");
        assert!(out.contains('╯'), "{out}");
        assert!(out.contains("think"), "{out}");
    }

    #[test]
    fn sequence_cross_head() {
        let out = plain("sequenceDiagram\n A-x B: lost");
        assert!(out.contains('×'), "{out}");
    }

    #[test]
    fn sequence_note_over_renders_box() {
        let out = plain("sequenceDiagram\n A->>B: hi\n Note over A,B: happy path");
        assert!(out.contains("happy path"), "{out}");
    }

    #[test]
    fn sequence_autonumber_prefixes_messages() {
        let out = plain("sequenceDiagram\n autonumber\n A->>B: one\n B->>A: two");
        assert!(out.contains("1. one"), "{out}");
        assert!(out.contains("2. two"), "{out}");
    }

    #[test]
    fn sequence_loop_renders_divider_and_end() {
        let out = plain("sequenceDiagram\n A->>B: hi\n loop retry x3\n A->>B: again\n end");
        assert!(out.contains("loop retry x3"), "{out}");
        assert!(out.contains(" end "), "{out}");
    }

    #[test]
    fn sequence_rect_block_is_invisible() {
        let out = plain("sequenceDiagram\n rect rgb(0,0,0)\n A->>B: hi\n end");
        assert!(!out.contains("rect"), "{out}");
        assert!(!out.contains(" end "), "rect end is silent:\n{out}");
    }

    #[test]
    fn sequence_box_end_does_not_close_enclosing_block() {
        let out = plain(
            "sequenceDiagram\n loop l1\n box g\n participant A\n end\n A->>B: hi\n A->>B: bye\n end",
        );
        assert_eq!(
            out.matches(" end ").count(),
            1,
            "box end is silent, loop end renders:\n{out}"
        );
        let lines: Vec<&str> = out.lines().collect();
        let row = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            row("loop l1") < row("hi") && row("bye") < row(" end "),
            "messages stay inside the loop:\n{out}"
        );
        assert!(!out.contains("box"), "{out}");
    }

    #[test]
    fn sequence_critical_option_renders_dividers() {
        let out = plain(
            "sequenceDiagram\n critical connect\n A->>B: try\n option timeout\n A->>A: log\n end",
        );
        assert!(
            out.contains("critical connect"),
            "valid critical diagram renders:\n{out}"
        );
        assert!(out.contains("option timeout"), "{out}");
        assert!(out.contains(" end "), "{out}");
    }

    #[test]
    fn sequence_long_label_widens_gap() {
        let out = plain(
            "sequenceDiagram\n A->>B: a very long message label that needs room\n B-->>A: ok",
        );
        assert!(
            out.contains("a very long message label that needs room"),
            "{out}"
        );
    }

    #[test]
    fn mixed_solid_and_dotted_bus_stays_light() {
        let out = plain("graph TD\n A --> C\n B -.-> C");
        assert!(out.contains('╌'), "dotted branch survives:\n{out}");
        assert!(out.contains('─'), "solid branch survives:\n{out}");
        assert!(out.contains('┬'), "shared merge cell stays light:\n{out}");
    }

    #[test]
    fn box_borders_stay_light_next_to_styled_edges() {
        let out = plain("graph TD\n A ==> B");
        assert!(out.contains('┌') && out.contains('└'), "{out}");
        assert!(!out.contains('┏'), "borders not restyled:\n{out}");
    }

    #[test]
    fn self_loop_renders_below_box() {
        let out = plain("graph TD\n A --> A");
        assert!(out.contains('╰') && out.contains('╯'), "{out}");
        assert!(out.contains('▲'), "loop returns into the box:\n{out}");
    }

    #[test]
    fn self_loop_label_renders() {
        let out = plain("graph TD\n A -->|again| A");
        assert!(out.contains("again"), "{out}");
    }

    #[test]
    fn self_loop_coexists_with_forward_edge() {
        let out = plain("graph TD\n A --> A\n A --> B");
        assert!(out.contains('▲'), "{out}");
        assert!(out.contains('▼'), "{out}");
        assert!(out.contains('B'), "{out}");
        assert!(!out.contains('┼'), "{out}");
    }

    #[test]
    fn self_loop_flips_with_bt() {
        let out = plain("flowchart BT\n A --> A\n A --> B");
        assert!(out.contains('▼'), "flipped loop head points down:\n{out}");
        assert!(out.contains('╭') || out.contains('╮'), "{out}");
    }

    #[test]
    fn self_loop_in_lr() {
        let out = plain("flowchart LR\n A --> A\n A --> B");
        assert!(out.contains('▲'), "{out}");
        assert!(out.contains('▶'), "{out}");
    }

    #[test]
    fn inline_o_word_label_still_parses_as_label() {
        let g = parse_graph("graph TD\n A -- or else --> B").unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.edges[0].label.as_deref(), Some("or else"));
    }

    #[test]
    fn sequence_unparseable_arrow_falls_back() {
        let out = plain("sequenceDiagram\n ->>B: orphan");
        assert!(out.contains("mermaid: sequenceDiagram"), "{out}");
    }

    #[test]
    fn sequence_unknown_statement_falls_back() {
        let out = plain("sequenceDiagram\n A->>B: hi\n garbage statement here");
        assert!(out.contains("mermaid: sequenceDiagram"), "{out}");
    }

    #[test]
    fn sequence_over_wide_falls_back() {
        let out = render(
            "sequenceDiagram\n A->>B: this label is far wider than the available pane width",
            &styles(),
            Some(30),
        )
        .unwrap()
        .plain_lines
        .join("\n");
        assert!(out.contains("mermaid: sequenceDiagram"), "{out}");
    }

    #[test]
    fn sequence_over_cap_falls_back() {
        let mut src = String::from("sequenceDiagram\n");
        for i in 0..600 {
            src.push_str(&format!(" A->>B: msg {i}\n"));
        }
        let out = plain(&src);
        assert!(out.contains("mermaid: sequenceDiagram"), "{out}");
    }

    #[test]
    fn sequence_activation_markers_are_stripped() {
        let out = plain("sequenceDiagram\n A->>+B: call\n B-->>-A: return");
        assert!(out.contains("call"), "{out}");
        assert!(out.contains("return"), "{out}");
        assert!(!out.contains('+'), "{out}");
    }

    #[test]
    fn sequence_rows_are_rectangular_and_sentinel_free() {
        let out = plain("sequenceDiagram\n Alice->>Bob: hi\n Note over Alice: solo note");
        assert!(!out.contains(CONT), "sentinel leaked:\n{out}");
        assert!(out.contains("solo note"), "{out}");
    }
