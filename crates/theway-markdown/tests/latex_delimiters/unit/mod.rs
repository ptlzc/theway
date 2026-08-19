    use super::*;

    fn norm(s: &str) -> String {
        normalize_latex_delimiters(s)
    }

    // ── Basic conversions ────────────────────────────────────────────────

    #[test]
    fn inline_paren_converts() {
        assert_eq!(norm("\\(x^2\\)"), "$x^2$");
        assert_eq!(norm("a \\(x\\) b"), "a $x$ b");
    }

    // ── Inline `\( … \)` boundary-whitespace trimming (the regression) ────

    #[test]
    fn normalize_inline_paren_trims_boundary_ws() {
        // Padding on both flanks is stripped so pulldown's dollar-math flanking
        // rule accepts the emitted `$…$`.
        assert_eq!(norm("a \\( x+y \\) b"), "a $x+y$ b");
        // One-sided padding.
        assert_eq!(norm("\\(x \\)"), "$x$");
        assert_eq!(norm("\\( x\\)"), "$x$");
        // Multiple spaces / tabs collapse away at the boundaries only.
        assert_eq!(norm("\\(   x+y   \\)"), "$x+y$");
        assert_eq!(norm("\\(\tx\t\\)"), "$x$");
        // VT (0x0B) is the one flanking-whitespace char `char::is_ascii_whitespace`
        // omits, so this pins the custom trim predicate against a regression to std.
        assert_eq!(norm("\\(\u{0b}x\u{0b}\\)"), "$x$");
        // Interior whitespace and inner escaped braces are preserved.
        assert_eq!(norm("\\( a + b \\)"), "$a + b$");
        assert_eq!(norm("\\( \\{x\\} \\)"), "$\\{x\\}$");
    }

    #[test]
    fn normalize_inline_paren_trim_leaves_escapes_and_dollars_alone() {
        // Escaped `\\(`/`\\)` is a literal backslash + paren, not a math span.
        assert_eq!(norm("\\\\( x \\\\)"), "\\\\( x \\\\)");
        // Only the backslash forms are ours: a space-padded bare `$ x $` is NOT
        // trimmed (currency untouched-ness is covered by `currency_not_misconverted`).
        assert_eq!(norm("$ x $"), "$ x $");
    }

    #[test]
    fn normalize_inline_paren_empty_span_degrades_position_for_position() {
        // A whitespace-only span keeps its interior between two lone `$` (the
        // old position-for-position form) rather than trimming to a `$$` opener.
        assert_eq!(norm("\\( \\)"), "$ $");
        assert_eq!(norm("\\(   \\)"), "$   $");
        // A truly-empty `\(\)` has no interior to separate the `$`, so it still
        // collapses to `$$` — matching the pre-fix behavior (pinned, not a goal).
        assert_eq!(norm("\\(\\)"), "$$");
    }

    #[test]
    fn display_bracket_converts() {
        assert_eq!(norm("\\[x^2\\]"), "$$x^2$$");
        assert_eq!(norm("a\n\\[x\\]\nb"), "a\n$$x$$\nb");
    }

    // ── Multi-line display spans join onto one line ──────────────────────

    #[test]
    fn multiline_display_with_setext_hazard_joins() {
        // A lone `=` line inside a display span is a CommonMark setext
        // underline: unjoined, pulldown parses a heading and the math is
        // never seen (the raw-LaTeX bug).
        assert_eq!(norm("$$\nx\n=\ny\n$$"), "$$x = y$$");
        assert_eq!(norm("\\[\nx\n=\ny\n\\]"), "$$x = y$$");
        // `-` (setext H2 / list marker) likewise.
        assert_eq!(norm("$$\na\n- b\n$$"), "$$a - b$$");
    }

    #[test]
    fn multiline_display_bracket_joins() {
        assert_eq!(norm("\\[\n\\frac{a+b}{2}\n\\]"), "$$\\frac{a+b}{2}$$");
        // Indented continuation lines are trimmed.
        assert_eq!(norm("$$\n  x +\n  y\n$$"), "$$x + y$$");
    }

    #[test]
    fn multiline_equation_env_joins() {
        assert_eq!(
            norm("\\begin{equation}\nE\n=\nmc^2\n\\end{equation}"),
            "$$E = mc^2$$"
        );
    }

    #[test]
    fn mismatched_display_delimiters_still_join() {
        // Every opener accepts every closer, matching the old behavior where
        // each token normalized to `$$` independently.
        assert_eq!(norm("\\[\nx\n=\ny\n$$"), "$$x = y$$");
        assert_eq!(norm("$$\nx\n\\]"), "$$x$$");
    }

    #[test]
    fn single_line_display_spans_unchanged() {
        assert_eq!(norm("$$x = y$$"), "$$x = y$$");
        assert_eq!(norm("text $$ a $$ more"), "text $$ a $$ more");
        // Whitespace inside single-line spans is preserved verbatim.
        assert_eq!(norm("$$  x  $$"), "$$  x  $$");
    }

    #[test]
    fn display_join_aborts_at_blank_line() {
        // Two stray `$$` across a paragraph break must not fuse into a span.
        let input = "Tickets cost $$.\n\nDinner cost $$.";
        assert_eq!(norm(input), input);
        // Math with an interior blank line stays as-is too (pre-existing
        // breakage; joining across paragraphs would be worse).
        let math = "$$\nx\n\ny\n$$";
        assert_eq!(norm(math), math);
    }

    #[test]
    fn display_join_aborts_at_blockquote_marker() {
        // Quoted display math keeps its `>` markers: pulldown strips them per
        // line and handles the span; joining would make them span content.
        let input = "> $$\n> x + y\n> $$";
        assert_eq!(norm(input), input);
    }

    #[test]
    fn unclosed_display_dollar_stays_literal() {
        assert_eq!(norm("a $$ x = y"), "a $$ x = y");
        assert_eq!(norm("$$"), "$$");
        // Triple-and-more dollar runs pass through verbatim.
        assert_eq!(norm("$$$"), "$$$");
        assert_eq!(norm("$$$$"), "$$$$");
    }

    #[test]
    fn display_join_gives_up_past_cap() {
        // No close within MAX_MATH_SOURCE_LEN: the opener stays literal and
        // the interior is processed normally.
        let big = "y".repeat(MAX_MATH_SOURCE_LEN + 10);
        let input = format!("$$\nx\n{big}");
        assert_eq!(norm(&input), input);
    }

    #[test]
    fn display_join_handles_crlf() {
        assert_eq!(norm("$$\r\nx\r\n=\r\ny\r\n$$"), "$$x = y$$");
    }

    #[test]
    fn interior_dollar_escapes_are_span_content() {
        assert_eq!(norm("$$\nprice \\$5\n=\nz\n$$"), "$$price \\$5 = z$$");
    }

    // ── Inline `\(…\)` spans join interior newlines ──────────────────────

    #[test]
    fn multiline_inline_paren_joins() {
        // A wrapped inline span is equally vulnerable to setext re-parsing.
        assert_eq!(norm("\\(a\n=\nb\\)"), "$a = b$");
        assert_eq!(norm("\\(x +\n  y\\)"), "$x + y$");
    }

    #[test]
    fn equation_env_converts() {
        assert_eq!(norm("\\begin{equation} x=1 \\end{equation}"), "$$ x=1 $$");
        assert_eq!(norm("\\begin{equation*} y \\end{equation*}"), "$$ y $$");
    }

    #[test]
    fn dollar_forms_unchanged() {
        assert_eq!(norm("$x$"), "$x$");
        assert_eq!(norm("$$x$$"), "$$x$$");
        assert_eq!(norm("text $a+b$ more"), "text $a+b$ more");
    }

    #[test]
    fn idempotent() {
        for s in [
            "\\(x\\)",
            "\\[y\\]",
            "a \\(x\\) and \\[y\\] and $z$",
            "\\begin{equation} q \\end{equation}",
            "`\\(code\\)`",
            "```\n\\(c\\)\n```\n",
            "$$\nx\n=\ny\n$$",
            "\\[\nx\n=\ny\n\\]",
            "\\begin{equation}\nE\n=\nmc^2\n\\end{equation}",
            "\\(a\n=\nb\\)",
            "a $$ x = y",
            "> $$\n> x\n> $$",
            "Tickets cost $$.\n\nDinner cost $$.",
        ] {
            let once = norm(s);
            let twice = norm(&once);
            assert_eq!(once, twice, "not idempotent for {s:?}");
        }
    }

    // ── Escapes & currency ───────────────────────────────────────────────

    #[test]
    fn escaped_backslash_paren_stays_literal() {
        // `\\(` = escaped backslash + literal paren → must NOT become math.
        assert_eq!(norm("\\\\(x\\\\)"), "\\\\(x\\\\)");
        // `\\\(` = escaped backslash + real `\(` → the `\(` converts.
        assert_eq!(norm("\\\\\\(x\\\\\\)"), "\\\\$x\\\\$");
    }

    #[test]
    fn escaped_dollar_stays_literal() {
        assert_eq!(norm("price \\$5"), "price \\$5");
    }

    #[test]
    fn currency_not_misconverted() {
        assert_eq!(norm("$5 and $10"), "$5 and $10");
        assert_eq!(norm("\\(a\\) costs $5"), "$a$ costs $5");
    }

    // ── Code is left verbatim ────────────────────────────────────────────

    #[test]
    fn inline_code_latex_untouched() {
        assert_eq!(norm("`\\(x\\)`"), "`\\(x\\)`");
        assert_eq!(norm("see `\\[y\\]` here"), "see `\\[y\\]` here");
        // Double-backtick code span with an embedded single backtick.
        assert_eq!(norm("``a ` \\(x\\)``"), "``a ` \\(x\\)``");
    }

    #[test]
    fn fenced_code_latex_untouched() {
        assert_eq!(norm("```\n\\(x\\)\n```\n"), "```\n\\(x\\)\n```\n");
        assert_eq!(norm("```latex\n\\[y\\]\n```\n"), "```latex\n\\[y\\]\n```\n");
        // Tilde fence.
        assert_eq!(norm("~~~\n\\(x\\)\n~~~\n"), "~~~\n\\(x\\)\n~~~\n");
    }

    #[test]
    fn math_around_code_still_converts() {
        assert_eq!(norm("\\(a\\) `code` \\(b\\)"), "$a$ `code` $b$");
        assert_eq!(
            norm("\\(a\\)\n```\nx\n```\n\\(b\\)"),
            "$a$\n```\nx\n```\n$b$"
        );
    }

    #[test]
    fn fence_with_three_space_indent() {
        assert_eq!(
            norm("   ```\n   \\(x\\)\n   ```\n"),
            "   ```\n   \\(x\\)\n   ```\n"
        );
    }

    // ── Math inside tables (the bug) ─────────────────────────────────────

    #[test]
    fn table_cell_backslash_math_converts() {
        let input = "| Mode | Metric |\n|---|---|\n| Open | Decay vs \\(L_{x}\\) |\n";
        let expected = "| Mode | Metric |\n|---|---|\n| Open | Decay vs $L_{x}$ |\n";
        assert_eq!(norm(input), expected);
    }

    // ── Streaming equivalence (the key invariant) ────────────────────────

    const RICH_DOC: &str = concat!(
        "Inline \\(a+b\\), dollar $c+d$, display \\[e=mc^2\\].\n\n",
        "Padded \\( x + y \\) and \\( \\alpha + \\beta \\) spans.\n\n",
        "| Col | Math |\n|---|---|\n| x | \\(\\alpha\\) | $\\beta$ |\n\n",
        "Code `\\(not math\\)` stays raw.\n\n",
        "```latex\n\\(also not\\)\n\\[block\\]\n```\n\n",
        "Env \\begin{equation} x=1 \\end{equation} done.\n\n",
        "Escaped \\\\(literal\\\\), price $5 and $10.\n",
        "List:\n- item \\(p\\to q\\)\n- plain\n\n",
        "> quote \\[E=mc^2\\]\n\n",
        "## Heading \\(h=x^3\\)\n",
    );

    fn assert_split_invariant(doc: &str) {
        let oneshot = norm(doc);

        // 2-way: split at every char boundary.
        for split in 0..=doc.len() {
            if !doc.is_char_boundary(split) {
                continue;
            }
            let mut nz = LatexDelimiterNormalizer::new();
            let mut got = nz.push(&doc[..split]);
            got.push_str(&nz.push(&doc[split..]));
            got.push_str(&nz.finish());
            assert_eq!(got, oneshot, "2-way split at byte {split}");
        }
    }

    fn assert_char_by_char(doc: &str) {
        let oneshot = norm(doc);
        let mut nz = LatexDelimiterNormalizer::new();
        let mut got = String::new();
        for ch in doc.chars() {
            got.push_str(&nz.push(ch.encode_utf8(&mut [0u8; 4])));
        }
        got.push_str(&nz.finish());
        assert_eq!(got, oneshot, "char-by-char stream");
    }

    #[test]
    fn streaming_matches_oneshot_all_splits() {
        assert_split_invariant(RICH_DOC);
    }

    #[test]
    fn streaming_matches_oneshot_char_by_char() {
        assert_char_by_char(RICH_DOC);
    }

    #[test]
    fn streaming_matches_oneshot_edge_fixtures() {
        for doc in [
            "\\(x\\)",
            "\\[x\\]",
            "\\begin{equation}z\\end{equation}",
            "trailing backslash \\",
            "ends with paren open \\(",
            " ambiguous \\beg",
            "backtick run at end ```",
            "  ",
            "\\\\(escaped\\\\)",
            "`unterminated \\(x\\)\nafter \\(y\\)",
            // Padded inline spans exercise the look-ahead + trim hold-back.
            "\\( x \\)",
            "a \\( x+y \\) b",
            "\\( \\alpha + \\beta \\)",
            "\\( \\{x\\} \\)",
            "\\( \\) empty",
            // Unclosed padded open: held back until finish() flushes a lone `$`.
            "unclosed padded \\( x + y",
            // Display spans exercise the close-scan hold-back and its aborts.
            "$$\nx\n=\ny\n$$",
            "\\[\n\\boxed{ x\n=\ny }\n\\]",
            "\\begin{equation}\na\n=\nb\n\\end{equation}",
            "$$\nx\n\ny\n$$",
            "> $$\n> x\n> $$",
            "a $$ unclosed",
            "$$$",
            "trailing dollars $$",
            "$$\r\nx\r\n$$",
            "\\(a\n=\nb\\)",
            "text $5 and $$ x $$ and $10",
        ] {
            assert_split_invariant(doc);
            assert_char_by_char(doc);
        }
    }

    // ── finish() flushes held-back partials literally ────────────────────

    #[test]
    fn finish_flushes_partial_backslash() {
        let mut nz = LatexDelimiterNormalizer::new();
        let mut got = nz.push("a\\");
        got.push_str(&nz.finish());
        assert_eq!(got, "a\\");
    }

    #[test]
    fn finish_flushes_partial_env() {
        let mut nz = LatexDelimiterNormalizer::new();
        let mut got = nz.push("x \\begin{eq");
        got.push_str(&nz.finish());
        assert_eq!(got, "x \\begin{eq");
    }

    #[test]
    fn reset_clears_state() {
        let mut nz = LatexDelimiterNormalizer::new();
        let _ = nz.push("```\ncode \\(x\\)");
        nz.reset();
        // After reset we are back in Normal at line start.
        let mut got = nz.push("\\(y\\)");
        got.push_str(&nz.finish());
        assert_eq!(got, "$y$");
    }

    #[test]
    fn trailing_closing_backtick_held_until_finish_repro_auto_wake() {
        let msg = "That was just a stale progress check finishing — no new work. \
The review is already complete at:\n\n\
`/tmp/project/results/report.html`";

        let mut nz = LatexDelimiterNormalizer::new();
        let pre_finish = nz.push(msg);
        assert!(
            !pre_finish.ends_with('`'),
            "pre-finish source must hold back the trailing closer; got {:?}",
            &pre_finish[pre_finish.len().saturating_sub(40)..]
        );
        assert!(
            pre_finish.contains('`') && pre_finish.contains("/tmp/project/results"),
            "opener + path should already be emitted"
        );

        let mut full = pre_finish;
        full.push_str(&nz.finish());
        assert!(
            full.ends_with('`'),
            "finish() must flush the held-back closing backtick"
        );
        assert_eq!(full, msg);

        let mut nz = LatexDelimiterNormalizer::new();
        let mut streamed = nz.push(&msg[..msg.len() - 1]);
        streamed.push_str(&nz.push("`"));
        assert!(
            !streamed.ends_with('`') || streamed.matches('`').count() < 2,
            "closing backtick still held after final chunk without finish(); got {:?}",
            &streamed[streamed.len().saturating_sub(40)..]
        );
        streamed.push_str(&nz.finish());
        assert_eq!(streamed, msg);
    }
