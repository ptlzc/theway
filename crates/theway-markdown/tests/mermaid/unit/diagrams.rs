    #[test]
    fn fan_out_keeps_single_bus_row() {
        let out = plain("graph TD\n A --> C[ccc]\n A --> D[ddd]");
        let baseline = plain("graph TD\n A --> C[ccc]");
        assert_eq!(
            out.lines().count(),
            baseline.lines().count(),
            "shared-source jogs share one bus row:\n{out}"
        );
        assert!(!out.contains('┼'), "{out}");
    }

    #[test]
    fn shared_target_back_edges_share_one_lane() {
        let two = plain("graph TD\n A --> B\n B --> C\n B --> A\n C --> A");
        let one = plain("graph TD\n A --> B\n B --> C\n C --> A");
        assert_eq!(
            two.lines().map(|l| l.width()).max(),
            one.lines().map(|l| l.width()).max(),
            "shared-target back edges merge into one lane:\n{two}"
        );
        assert_eq!(two.matches('◄').count(), 1, "{two}");
    }

    #[test]
    fn distinct_back_edges_get_separate_lanes() {
        let split = plain("graph TD\n A --> B\n B --> C\n B --> A\n C --> B");
        let single = plain("graph TD\n A --> B\n B --> C\n C --> B");
        assert_eq!(split.matches('◄').count(), 2, "{split}");
        assert!(
            split.lines().map(|l| l.width()).max() > single.lines().map(|l| l.width()).max(),
            "overlapping unrelated back edges claim a second lane:\n{split}"
        );
    }

    #[test]
    fn fallback_wraps_long_lines_to_max_width() {
        let out = render(
            "gantt\n title a very long line that should wrap inside the fallback box nicely",
            &styles(),
            Some(40),
        )
        .unwrap()
        .plain_lines;
        assert!(out.iter().all(|l| l.width() <= 40), "{}", out.join("\n"));
        for line in &out[1..out.len() - 1] {
            assert!(
                line.starts_with('│') && line.ends_with('│'),
                "body rows keep both borders: {line:?}"
            );
        }
        assert!(out.join("\n").contains("nicely"), "{}", out.join("\n"));
    }

    #[test]
    fn class_renders_compartments() {
        let out = plain(
            "classDiagram\n class Animal {\n +int age\n +isMammal() bool\n }\n Animal <|-- Duck",
        );
        assert!(out.contains("Animal"), "{out}");
        assert!(out.contains("+int age"), "{out}");
        assert!(out.contains("+isMammal() bool"), "{out}");
        assert!(
            out.contains('├') && out.contains('┤'),
            "section rules:\n{out}"
        );
        let lines: Vec<&str> = out.lines().collect();
        let name = lines.iter().position(|l| l.contains("Animal")).unwrap();
        let attr = lines.iter().position(|l| l.contains("+int age")).unwrap();
        let method = lines
            .iter()
            .position(|l| l.contains("+isMammal() bool"))
            .unwrap();
        assert!(name < attr && attr < method, "{out}");
    }

    #[test]
    fn class_inheritance_triangle_at_parent() {
        let out = plain("classDiagram\n Animal <|-- Duck\n Animal <|-- Fish");
        assert!(out.contains('△'), "hollow triangle:\n{out}");
        let lines: Vec<&str> = out.lines().collect();
        let animal = lines.iter().position(|l| l.contains("Animal")).unwrap();
        let duck = lines.iter().position(|l| l.contains("Duck")).unwrap();
        assert!(animal < duck, "parent above child:\n{out}");
        let tri = lines.iter().position(|l| l.contains('△')).unwrap();
        assert!(
            tri >= animal && tri < duck,
            "triangle at parent end:\n{out}"
        );
    }

    #[test]
    fn class_realization_is_dotted_triangle() {
        let g = parse_class("classDiagram\n IShape <|.. Circle").unwrap().0;
        assert_eq!(g.edges[0].head_from, Head::Triangle);
        assert!(g.edges[0].line == LineKind::Dotted);
        let out = plain("classDiagram\n IShape <|.. Circle");
        assert!(out.contains('╎') || out.contains('╌'), "{out}");
    }

    #[test]
    fn class_composition_and_aggregation_diamonds() {
        let out = plain("classDiagram\n Car *-- Engine\n Pond o-- Duck");
        assert!(out.contains('◆'), "filled diamond:\n{out}");
        assert!(out.contains('◇'), "open diamond:\n{out}");
    }

    #[test]
    fn class_dependency_dotted_arrow() {
        let g = parse_class("classDiagram\n A ..> B").unwrap().0;
        assert_eq!(g.edges[0].head_to, Head::Arrow);
        assert!(g.edges[0].line == LineKind::Dotted);
    }

    #[test]
    fn class_colon_members_merge_with_block() {
        let out = plain(
            "classDiagram\n class Duck {\n +swim()\n }\n Duck : +String beakColor\n S --> Duck",
        );
        assert!(out.contains("+swim()"), "{out}");
        assert!(out.contains("+String beakColor"), "{out}");
    }

    #[test]
    fn class_annotation_renders_guillemets() {
        let out = plain("classDiagram\n <<interface>> Shape\n Shape <|.. Circle");
        assert!(out.contains("«interface»"), "{out}");
    }

    #[test]
    fn class_generics_display_as_angle_brackets() {
        let out = plain("classDiagram\n Shape~T~ : +area() T\n S --> Shape~T~");
        assert!(out.contains("Shape<T>"), "{out}");
        assert!(!out.contains('~'), "{out}");
    }

    #[test]
    fn class_cardinalities_fold_into_label() {
        let out = plain("classDiagram\n Student \"many\" --> \"1\" School : attends");
        assert!(out.contains("many attends 1"), "{out}");
    }

    #[test]
    fn class_from_end_head_survives_fan_out_jog() {
        let out = plain("classDiagram\n Animal <|-- Duck\n Animal <|-- Fish\n Animal <|-- Cow");
        assert_eq!(
            out.matches('△').count() + out.matches('▽').count(),
            1,
            "merged from-end glyph on the parent border:\n{out}"
        );
    }

    #[test]
    fn class_empty_class_is_plain_titled_box() {
        let out = plain("classDiagram\n class Loner\n A --> Loner");
        assert!(out.contains("Loner"), "{out}");
    }

    #[test]
    fn class_unknown_statement_falls_back() {
        let out = plain("classDiagram\n A --> B\n total garbage here");
        assert!(out.contains("mermaid: classDiagram"), "{out}");
    }

    #[test]
    fn class_member_cap_ellipsis() {
        let mut src = String::from("classDiagram\n class Big {\n");
        for i in 0..12 {
            src.push_str(&format!(" +field{i}\n"));
        }
        src.push_str(" }\n A --> Big");
        let out = plain(&src);
        assert!(out.contains("+field7"), "{out}");
        assert!(!out.contains("+field9"), "{out}");
        assert!(out.contains('…'), "{out}");
    }

    #[test]
    fn class_direction_lr() {
        let out = plain("classDiagram\n direction LR\n A --> B");
        let line = out.lines().find(|l| l.contains('A')).unwrap();
        assert!(line.contains('B'), "{out}");
    }

    #[test]
    fn er_renders_entities_and_relationship_labels() {
        let out = plain(
            "erDiagram\n CUSTOMER ||--o{ ORDER : places\n CUSTOMER {\n string name PK \"full name\"\n int custNumber\n }",
        );
        assert!(out.contains("CUSTOMER"), "{out}");
        assert!(out.contains("ORDER"), "{out}");
        assert!(out.contains("string name PK"), "{out}");
        assert!(
            !out.contains("full name"),
            "attribute comments dropped:\n{out}"
        );
        assert!(out.contains("1 places 0..*"), "{out}");
        assert!(out.contains('├'), "attribute compartment rule:\n{out}");
    }

    #[test]
    fn er_cardinality_map() {
        let cases = [
            ("||--||", "1", "1"),
            ("|o--o|", "0..1", "0..1"),
            ("}o--o{", "0..*", "0..*"),
            ("}|--|{", "1..*", "1..*"),
            ("||--o{", "1", "0..*"),
        ];
        for (op, l, r) in cases {
            let (cl, cr, line) = parse_er_op(op).unwrap();
            assert_eq!((cl, cr), (l, r), "{op}");
            assert!(line == LineKind::Solid);
        }
        assert!(parse_er_op("||..o{").unwrap().2 == LineKind::Dotted);
        assert!(parse_er_op("||==o{").is_none());
        assert!(parse_er_op("garbage").is_none());
    }

    #[test]
    fn er_non_identifying_renders_dotted() {
        let out = plain("erDiagram\n A ||..o{ B : uses");
        assert!(out.contains('╎') || out.contains('╌'), "{out}");
    }

    #[test]
    fn er_relationships_have_no_arrowheads() {
        let out = plain("erDiagram\n A ||--o{ B : has");
        for head in ['▼', '▲', '◄', '▶', '△', '◆', '◇'] {
            assert!(!out.contains(head), "{head} in:\n{out}");
        }
    }

    #[test]
    fn er_entity_alias_label() {
        let out = plain("erDiagram\n p[Person] ||--o{ a[\"Bank Account\"] : owns");
        assert!(out.contains("Person"), "{out}");
        assert!(out.contains("Bank Account"), "{out}");
    }

    #[test]
    fn er_unquoted_label_and_bare_entity_decl() {
        let g = parse_er("erDiagram\n LONER\n A ||--|| B : linked")
            .unwrap()
            .0;
        assert_eq!(g.nodes.len(), 3);
        let out = plain("erDiagram\n LONER\n A ||--|| B : linked");
        assert!(out.contains("LONER"), "{out}");
        assert!(out.contains("1 linked 1"), "{out}");
    }

    #[test]
    fn er_attribute_cap_ellipsis() {
        let mut src = String::from("erDiagram\n BIG {\n");
        for i in 0..12 {
            src.push_str(&format!(" int f{i}\n"));
        }
        src.push_str(" }\n BIG ||--|| OTHER : x");
        let out = plain(&src);
        assert!(out.contains("int f7"), "{out}");
        assert!(!out.contains("int f9"), "{out}");
        assert!(out.contains('…'), "{out}");
    }

    #[test]
    fn er_unknown_statement_falls_back() {
        let out = plain("erDiagram\n A ||--|| B : ok\n utter nonsense statement");
        assert!(out.contains("mermaid: erDiagram"), "{out}");
    }

    #[test]
    fn subgraph_renders_titled_frame() {
        let out = plain(
            "graph TD\n S[Start] --> one\n subgraph one [Group One]\n A --> B\n end\n one --> E[End]",
        );
        assert!(out.contains(" Group One "), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("Group One")).unwrap();
        let a = lines.iter().position(|l| l.contains("│ A │")).unwrap();
        let b = lines.iter().position(|l| l.contains("│ B │")).unwrap();
        let frame_close = lines
            .iter()
            .rposition(|l| l.trim_start().starts_with('└'))
            .unwrap();
        assert!(title < a && a < b && b <= frame_close, "{out}");
        assert!(out.contains("Start") && out.contains("End"), "{out}");
        assert_eq!(out.matches('▼').count(), 3, "{out}");
    }

    #[test]
    fn subgraph_edge_between_groups() {
        let out = plain(
            "graph TD\n subgraph api [API]\n A1 --> A2\n end\n subgraph db [Storage]\n B1\n end\n api --> db",
        );
        assert!(out.contains(" API "), "{out}");
        assert!(out.contains(" Storage "), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        let api = lines.iter().position(|l| l.contains("API")).unwrap();
        let db = lines.iter().position(|l| l.contains("Storage")).unwrap();
        assert!(api < db, "API frame ranks above Storage:\n{out}");
    }

    #[test]
    fn subgraph_nested_frames() {
        let out = plain(
            "graph TD\n subgraph outer [Outer]\n subgraph inner [Inner]\n X --> Y\n end\n W --> X\n end\n S --> outer",
        );
        assert!(out.contains(" Outer "), "{out}");
        assert!(out.contains(" Inner "), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        let outer = lines.iter().position(|l| l.contains("Outer")).unwrap();
        let inner = lines.iter().position(|l| l.contains("Inner")).unwrap();
        assert!(outer < inner, "{out}");
    }

    #[test]
    fn subgraph_cross_member_edge_attaches_to_frame() {
        let out = plain("graph LR\n S --> A\n subgraph g [Workers]\n A --> B\n end\n B --> T");
        assert!(out.contains(" Workers "), "{out}");
        assert!(out.contains('S') && out.contains('T'), "{out}");
        assert_eq!(out.matches('▶').count(), 3, "{out}");
        let row = out.lines().find(|l| l.contains("│ A ├")).unwrap();
        assert!(
            row.find('S') < row.find('A'),
            "A stays outside the group (first definition wins):\n{out}"
        );
    }

    #[test]
    fn subgraph_id_referenced_before_declaration() {
        let g = parse_graph("graph TD\n X --> two\n subgraph two\n C --> D\n end").unwrap();
        assert_eq!(g.groups.len(), 1);
        let out = plain("graph TD\n X --> two\n subgraph two\n C --> D\n end");
        assert!(out.contains(" two "), "frame titled by id:\n{out}");
        assert!(out.contains("│ C │"), "{out}");
    }

    #[test]
    fn subgraph_quoted_and_plain_titles() {
        let out = plain("graph TD\n subgraph \"My Stuff\"\n A\n end\n S --> A");
        assert!(out.contains(" My Stuff "), "{out}");
        let out2 = plain("graph TD\n subgraph batch jobs\n B\n end\n S --> B");
        assert!(out2.contains(" batch jobs "), "{out2}");
        let out3 = plain("graph TD\n subgraph \"a &lt;b&gt;\"\n C\n end\n S --> C");
        assert!(out3.contains("a <b>") && !out3.contains("&lt;"), "{out3}");
    }

    #[test]
    fn subgraph_empty_is_dropped() {
        let out = plain("graph TD\n subgraph ghost\n end\n A --> B");
        assert!(!out.contains("ghost"), "{out}");
        assert!(out.contains('▼'), "{out}");
    }

    #[test]
    fn subgraph_bt_flips_frame_and_contents() {
        let out = plain("flowchart BT\n S --> one\n subgraph one [Up]\n A --> B\n end");
        assert!(out.contains(" Up "), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        let row = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(row("│ B │") < row("│ A │"), "contents flip with BT:\n{out}");
        assert!(row(" Up ") < row("S"), "frame above source in BT:\n{out}");
        assert!(out.contains('▲'), "{out}");
    }

    #[test]
    fn subgraph_depth_over_cap_falls_back() {
        let mut src = String::from("graph TD\n");
        for i in 0..8 {
            src.push_str(&format!(" subgraph g{i}\n"));
        }
        src.push_str(" A --> B\n");
        for _ in 0..8 {
            src.push_str(" end\n");
        }
        let out = plain(&src);
        assert!(out.contains("mermaid: graph"), "{out}");
    }

    #[test]
    fn subgraph_groupless_path_unchanged() {
        let g = parse_graph("graph TD\n A --> B").unwrap();
        assert!(g.groups.is_empty());
    }

    #[test]
    fn fan_out_creates_cross_product_edges() {
        let g = parse_graph("graph TD\n A & B --> C & D").unwrap();
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges.len(), 4);
        let idx = |id: &str| g.index[id];
        let has = |f: &str, t: &str| g.edges.iter().any(|e| e.from == idx(f) && e.to == idx(t));
        assert!(has("A", "C") && has("A", "D") && has("B", "C") && has("B", "D"));
        let out = plain("graph TD\n A & B --> C & D");
        assert_eq!(out.chars().filter(|&c| c == '▼').count(), 2, "{out}");
    }

    #[test]
    fn fan_out_in_chain() {
        let g = parse_graph("graph LR\n A & B --> C --> D").unwrap();
        assert_eq!(g.edges.len(), 3);
    }

    #[test]
    fn fan_out_with_reversed_arrow() {
        let g = parse_graph("graph TD\n A & B <-- C").unwrap();
        let idx = |id: &str| g.index[id];
        assert_eq!(g.edges.len(), 2);
        assert!(g.edges.iter().all(|e| e.from == idx("C")));
        assert!(g.edges.iter().all(|e| e.head_to == Head::Arrow));
    }

    #[test]
    fn circle_and_cross_endings_create_no_phantom_nodes() {
        let g = parse_graph("graph TD\n A --o B\n C --x D").unwrap();
        assert_eq!(g.nodes.len(), 4, "no phantom o/x nodes");
        assert!(!g.index.contains_key("o"));
        assert!(!g.index.contains_key("x"));
        assert_eq!(g.edges[0].head_to, Head::Circle);
        assert_eq!(g.edges[1].head_to, Head::Cross);
        let out = plain("graph TD\n A --o B");
        assert!(out.contains('o'), "circle head rendered:\n{out}");
    }

    #[test]
    fn left_endings_decorate_without_reversing() {
        let g = parse_graph("graph TD\n A o-- B\n C x-- D").unwrap();
        let idx = |id: &str| g.index[id];
        assert_eq!(g.edges[0].from, idx("A"));
        assert_eq!(g.edges[0].to, idx("B"));
        assert_eq!(g.edges[0].head_from, Head::Circle);
        assert_eq!(g.edges[1].head_from, Head::Cross);
        assert_eq!(g.edges[0].head_to, Head::None);
    }

    #[test]
    fn reversed_arrow_with_end_marker_swaps_direction() {
        let g = parse_graph("graph TD\n A <--o B\n C <--x D").unwrap();
        let idx = |id: &str| g.index[id];
        assert_eq!(g.edges[0].from, idx("B"));
        assert_eq!(g.edges[0].to, idx("A"));
        assert_eq!(g.edges[0].head_to, Head::Arrow);
        assert_eq!(g.edges[0].head_from, Head::Circle);
        assert_eq!(g.edges[1].from, idx("D"));
        assert_eq!(g.edges[1].to, idx("C"));
        assert_eq!(g.edges[1].head_from, Head::Cross);
        let plain_rev = plain("graph TD\n A <--o B");
        let lines: Vec<&str> = plain_rev.lines().collect();
        let row = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            row("B") < row("A"),
            "ranks match plain <-- reversal:\n{plain_rev}"
        );
    }

    #[test]
    fn both_end_markers_parse() {
        let g = parse_graph("graph TD\n A o--o B\n C x--x D").unwrap();
        assert_eq!(g.edges[0].head_from, Head::Circle);
        assert_eq!(g.edges[0].head_to, Head::Circle);
        assert_eq!(g.edges[1].head_from, Head::Cross);
        assert_eq!(g.edges[1].head_to, Head::Cross);
        assert_eq!(g.nodes.len(), 4);
    }

    #[test]
    fn dotted_and_thick_lines_render_distinctly() {
        let dotted = plain("graph TD\n A -.-> B");
        assert!(dotted.contains('╎'), "dotted vertical:\n{dotted}");
        let thick = plain("graph TD\n A ==> B");
        assert!(thick.contains('┃'), "thick vertical:\n{thick}");
        let solid = plain("graph TD\n A --> B");
        assert!(
            !solid.contains('╎') && !solid.contains('┃'),
            "solid unchanged:\n{solid}"
        );
    }

    #[test]
    fn dotted_label_form_renders_dashed() {
        let out = plain("graph LR\n A -. maybe .-> B");
        assert!(out.contains('╌'), "{out}");
        assert!(out.contains("maybe"), "{out}");
    }

    #[test]
    fn thick_jog_uses_thick_corners() {
        let out = plain("graph TD\n A[aaaaaaa] ==> B\n A ==> C[ccccccc]");
        assert!(
            out.contains('┏') || out.contains('┓') || out.contains('┳'),
            "thick corners on jog:\n{out}"
        );
    }

    #[test]
    fn state_diagram_renders_states_and_transitions() {
        let out =
            plain("stateDiagram-v2\n [*] --> Idle\n Idle --> Running: start\n Running --> [*]");
        assert!(out.contains("Idle"), "{out}");
        assert!(out.contains("Running"), "{out}");
        assert!(out.contains("start"), "{out}");
        assert!(out.contains('▼'), "{out}");
        assert_eq!(
            out.matches('●').count(),
            2,
            "distinct start and end markers:\n{out}"
        );
        let lines: Vec<&str> = out.lines().collect();
        let first_dot = lines.iter().position(|l| l.contains('●')).unwrap();
        let last_dot = lines.iter().rposition(|l| l.contains('●')).unwrap();
        let idle = lines.iter().position(|l| l.contains("Idle")).unwrap();
        assert!(first_dot < idle && idle < last_dot, "{out}");
    }

    #[test]
    fn state_v1_header_renders() {
        let out = plain("stateDiagram\n A --> B");
        assert!(out.contains('▼'), "{out}");
    }

    #[test]
    fn state_boxes_are_rounded() {
        let out = plain("stateDiagram-v2\n A --> B");
        assert!(out.contains('╭'), "{out}");
        assert!(!out.contains('┌'), "states render rounded:\n{out}");
    }

    #[test]
    fn state_alias_label_renders() {
        let out = plain("stateDiagram-v2\n state \"Waiting for input\" as W\n W --> Done");
        assert!(out.contains("Waiting for input"), "{out}");
    }

    #[test]
    fn state_choice_parses_as_diamond() {
        let g = parse_state(
            "stateDiagram-v2\n state c <<choice>>\n A --> c\n c --> B: yes\n c --> D: no",
        )
        .unwrap();
        assert!(g.nodes[g.index["c"]].shape == Shape::Diamond);
        assert_eq!(g.edges.len(), 3);
    }

    #[test]
    fn state_description_sets_label() {
        let out = plain("stateDiagram-v2\n s2 : waits patiently\n A --> s2");
        assert!(out.contains("waits patiently"), "{out}");
    }

    #[test]
    fn state_direction_lr() {
        let out = plain("stateDiagram-v2\n direction LR\n A --> B --> C");
        let td = plain("stateDiagram-v2\n A --> B");
        assert!(
            out.lines().count() <= td.lines().count() + 2,
            "LR stays flat:\n{out}"
        );
        let line = out.lines().find(|l| l.contains('A')).unwrap();
        assert!(line.contains('B'), "A and B share a row in LR:\n{out}");
    }

    #[test]
    fn state_composite_contents_render_flat() {
        let out = plain("stateDiagram-v2\n state Active {\n A --> B\n }\n Active --> Done");
        assert!(out.contains("Active"), "{out}");
        assert!(out.contains('A') && out.contains('B'), "{out}");
        assert!(out.contains("Done"), "{out}");
    }

    #[test]
    fn state_notes_are_skipped() {
        let out = plain(
            "stateDiagram-v2\n A --> B\n note right of A: inline note\n note left of B\n block text\n end note",
        );
        assert!(out.contains('▼'), "{out}");
        assert!(!out.contains("note"), "{out}");
        assert!(!out.contains("block text"), "{out}");
    }

    #[test]
    fn state_back_transition_uses_lane() {
        let out = plain("stateDiagram-v2\n A --> B\n B --> C\n C --> B: retry");
        assert!(out.contains('◄'), "{out}");
        assert!(out.contains("retry"), "{out}");
    }

    #[test]
    fn state_unknown_statement_falls_back() {
        let out = plain("stateDiagram-v2\n A --> B\n some garbage line");
        assert!(out.contains("mermaid: stateDiagram-v2"), "{out}");
    }

    #[test]
    fn state_over_cap_falls_back() {
        let mut src = String::from("stateDiagram-v2\n");
        for i in 0..600 {
            src.push_str(&format!(" S{i} --> S{}\n", i + 1));
        }
        let out = plain(&src);
        assert!(out.contains("mermaid: stateDiagram-v2"), "{out}");
    }

    #[test]
    fn state_extra_dash_arrow_tolerated() {
        let g = parse_state("stateDiagram-v2\n A ---> B").unwrap();
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn state_description_preserves_choice_shape() {
        let g = parse_state(
            "stateDiagram-v2\n state c <<choice>>\n c : pick a path\n A --> c\n c --> B",
        )
        .unwrap();
        assert!(g.nodes[g.index["c"]].shape == Shape::Diamond);
        assert_eq!(g.nodes[g.index["c"]].label, "pick a path");
        let g2 =
            parse_state("stateDiagram-v2\n state c <<choice>>\n state \"pick\" as c\n A --> c")
                .unwrap();
        assert!(g2.nodes[g2.index["c"]].shape == Shape::Diamond);
        assert_eq!(g2.nodes[g2.index["c"]].label, "pick");
    }

    #[test]
    fn state_chained_transitions_parse_as_separate_edges() {
        let g = parse_state("stateDiagram-v2\n A --> B --> C").unwrap();
        assert_eq!(g.nodes.len(), 3, "three distinct states");
        assert_eq!(g.edges.len(), 2, "two edges");
        assert!(g.index.contains_key("B"));
        assert!(g.index.contains_key("C"));
        assert!(
            !g.nodes.iter().any(|n| n.label.contains("-->")),
            "no node swallows the arrow"
        );
        let idx = |id: &str| g.index[id];
        assert!(
            g.edges
                .iter()
                .any(|e| e.from == idx("A") && e.to == idx("B"))
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.from == idx("B") && e.to == idx("C"))
        );
    }

    #[test]
    fn state_chain_with_markers_and_label() {
        let g = parse_state("stateDiagram-v2\n [*] --> A --> B: done").unwrap();
        assert_eq!(g.edges.len(), 2);
        assert!(g.edges.iter().any(|e| e.label.as_deref() == Some("done")));
        let out = plain("stateDiagram-v2\n [*] --> A --> B: done");
        assert!(out.contains('●'), "{out}");
        assert!(out.contains("done"), "{out}");
    }

    #[test]
    fn state_dangling_chain_falls_back() {
        let out = plain("stateDiagram-v2\n A --> B -->");
        assert!(out.contains("mermaid: stateDiagram-v2"), "{out}");
    }

    #[test]
    fn sequence_renders_actors_and_messages() {
        let out = plain("sequenceDiagram\n Alice->>Bob: Hello Bob\n Bob-->>Alice: Hi Alice");
        assert!(out.contains("Alice"), "{out}");
        assert!(out.contains("Bob"), "{out}");
        assert!(out.contains("Hello Bob"), "{out}");
        assert!(out.contains('▶'), "solid call arrow:\n{out}");
        assert!(out.contains('◄'), "reply arrow:\n{out}");
        assert!(out.contains('╌'), "reply line is dashed:\n{out}");
        assert_eq!(
            out.matches("│ Alice │").count(),
            2,
            "actor boxes repeat at bottom:\n{out}"
        );
    }

    #[test]
    fn sequence_participant_as_label() {
        let out = plain(
            "sequenceDiagram\n participant C as Client\n participant S as Server\n C->>S: GET /",
        );
        assert!(out.contains("Client"), "{out}");
        assert!(out.contains("Server"), "{out}");
    }
