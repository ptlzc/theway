    use super::*;
    use crate::style::test_style;

    // Tests for count_trailing_blank_lines helper

    #[test]
    fn test_count_trailing_blank_lines_empty() {
        assert_eq!(count_trailing_blank_lines(""), 0);
    }

    #[test]
    fn test_count_trailing_blank_lines_no_newline() {
        assert_eq!(count_trailing_blank_lines("hello"), 0);
    }

    #[test]
    fn test_count_trailing_blank_lines_single_newline() {
        // Just a line ending, not a blank line
        assert_eq!(count_trailing_blank_lines("hello\n"), 0);
    }

    #[test]
    fn test_count_trailing_blank_lines_double_newline() {
        // One blank line (standard markdown block separator)
        assert_eq!(count_trailing_blank_lines("hello\n\n"), 1);
    }

    #[test]
    fn test_count_trailing_blank_lines_triple_newline() {
        // Two blank lines
        assert_eq!(count_trailing_blank_lines("hello\n\n\n"), 2);
    }

    #[test]
    fn test_count_trailing_blank_lines_quadruple_newline() {
        assert_eq!(count_trailing_blank_lines("hello\n\n\n\n"), 3);
    }

    #[test]
    fn test_count_trailing_blank_lines_with_spaces() {
        // Whitespace-only lines count as blank
        assert_eq!(count_trailing_blank_lines("hello\n  \n"), 1);
        assert_eq!(count_trailing_blank_lines("hello\n\t\n"), 1);
        assert_eq!(count_trailing_blank_lines("hello\n  \n  \n"), 2);
    }

    #[test]
    fn test_count_trailing_blank_lines_heading() {
        // Common markdown pattern: heading followed by blank line
        assert_eq!(count_trailing_blank_lines("# Heading\n\n"), 1);
        assert_eq!(count_trailing_blank_lines("# Heading\n\n\n"), 2);
    }

    // Basic Functionality Tests

    #[test]
    fn test_empty_renderer() {
        let renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        let output = renderer.view();
        assert!(output.lines.is_empty());
    }

    #[test]
    fn test_single_chunk() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render("# Hello\n\n", None);
        let output = renderer.view();
        assert!(!output.lines.is_empty());
    }

    #[test]
    fn test_multiple_chunks() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render("# Title\n\n", None);
        let out1_lines = renderer.view().lines.len();

        renderer.push_and_render("Some text\n\n", None);
        let out2_lines = renderer.view().lines.len();

        assert!(out2_lines >= out1_lines);
    }

    #[test]
    fn test_streaming_incomplete_paragraph() {
        // Test that incomplete paragraphs produce visible output
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        // First chunk: complete heading
        renderer.push_and_render("# Heading\n\n", None);
        let heading_lines = renderer.view().lines.len();
        assert!(heading_lines > 0, "Heading should produce lines");

        // Second chunk: start of paragraph (no newline)
        renderer.push_and_render("This is text ", None);
        let after_text_lines = renderer.view().lines.len();
        assert!(
            after_text_lines > heading_lines,
            "Incomplete paragraph should produce lines. Got {} lines, expected > {}",
            after_text_lines,
            heading_lines
        );

        // Third chunk: more text
        renderer.push_and_render("more text", None);
        let after_more_lines = renderer.view().lines.len();
        assert!(
            after_more_lines >= after_text_lines,
            "More text should not reduce lines. Got {}, expected >= {}",
            after_more_lines,
            after_text_lines
        );
    }

    #[test]
    fn test_freezing_occurs() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render("# Heading\n\n", None);
        // push() now renders automatically

        assert!(
            renderer.frozen_bytes() > 0,
            "Should freeze after complete heading"
        );
    }

    #[test]
    fn test_clear_resets_state() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render("# Hello\n\n", None);
        renderer.set_max_table_width(Some(80));

        renderer.clear();

        assert_eq!(renderer.source(), "");
        assert_eq!(renderer.frozen_bytes(), 0);
        assert_eq!(renderer.frozen_lines_count(), 0);

        // `clear()` must also reset `max_table_width`: otherwise a
        // subsequent `set_max_table_width(prev_value)` is silently a
        // no-op (the inner equality check sees no change), and the
        // expected reset behaviour disappears.  Verify the invariant
        // observationally — push content, observe a frozen state, then
        // re-set the prior width; the reset must wipe frozen state.
        renderer.push_and_render("# Heading\n\n", None);
        assert!(
            renderer.frozen_lines_count() > 0,
            "test setup: a complete heading should produce frozen lines",
        );
        // If clear() left max_table_width = Some(80), this call would
        // be a no-op and frozen_lines_count would stay > 0.
        renderer.set_max_table_width(Some(80));
        assert_eq!(
            renderer.frozen_lines_count(),
            0,
            "set_max_table_width(prev) after clear() must trigger a reset",
        );
    }

    #[test]
    fn test_finish_produces_full_render() {
        // Test that finish() produces identical output to full render
        let chunks = &["# Heading\n\n", "Some **bold** text.\n\n", "> Quote\n\n"];
        let full_text: String = chunks.iter().copied().collect();

        // Get full render for comparison
        let (full_output, _) =
            render_markdown_ratatui_full(&full_text, test_style::STYLE, true, None);
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Stream the content
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in chunks {
            renderer.push_and_render(chunk, None);
            // push() now renders automatically
        }

        // After finish - should be identical to full render
        let finished = renderer.finish(None);
        let after_finish: Vec<String> = finished
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(
            after_finish, full_lines,
            "finish() should produce identical output to full render"
        );

        // Verify frozen state is updated
        assert_eq!(renderer.frozen.source_bytes, full_text.len());
        assert_eq!(renderer.frozen.lines_len, full_lines.len());
    }

    #[test]
    fn streaming_without_finish_drops_trailing_inline_code_closer() {
        let msg = "already complete at:\n\n\
`/tmp/project/results/report.html`";

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render(msg, None);

        let pre = renderer.source().to_string();
        assert!(
            !pre.ends_with('`'),
            "without finish(), trailing closer must still be held back; source ends {:?}",
            &pre[pre.len().saturating_sub(20)..]
        );
        assert!(
            pre.contains('`') && pre.contains("report.html"),
            "opener + path should be present before finish"
        );

        let lines_pre: Vec<String> = renderer
            .view()
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let joined_pre = lines_pre.join("\n");
        assert!(
            joined_pre.contains('`'),
            "pre-finish render should still show a backtick (unclosed span); got {joined_pre:?}"
        );

        renderer.finish(None);
        let post = renderer.source().to_string();
        assert_eq!(
            post, msg,
            "finish() flushes the held-back closer into source"
        );
    }

    // Correctness Tests - Streaming vs Full Render

    /// Comprehensive markdown document covering many edge cases.
    const COMPREHENSIVE_MARKDOWN: &str = r#"# Main Heading

This is a paragraph with **bold**, *italic*, and `inline code`.

## Code Blocks

```rust
fn main() {
    println!("Hello, world!");
}
```

```
plain code
```

## Lists

- Item one
- Item two with **bold**

1. First
2. Second

- Outer item
  - Nested item 1
  - Nested item 2

## Blockquotes

> This is a blockquote.
> It spans multiple lines.

> Nested quote:
> > Inner quote

## Tables

| Column A | Column B | Column C |
|----------|:--------:|---------:|
| Left     | Center   | Right    |

## Mixed Content

- Step one
  ```python
  print("hello")
  ```
- Step two

> Some quote:
> - Quoted item 1
> - Quoted item 2

## Links and Images

Here's a [link](https://example.com) and another [one](https://test.com "with title").

## Thematic Breaks

Above the break.

---

Below the break.

## Edge Cases

Inline elements: ***bold italic*** and ~~strikethrough~~.

Final paragraph with no trailing newline."#;

    /// Stream character by character and compare final output.
    /// Uses push_raw() to batch all characters, then update() once at end.
    #[track_caller]
    fn assert_streaming_matches_full(text: &str, pretty: bool) {
        let mode = if pretty { "pretty" } else { "raw" };
        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, pretty, None);

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, pretty);
        for ch in text.chars() {
            renderer.push(&ch.to_string());
        }
        renderer.render(None);
        let streaming_output = renderer.view();

        assert_eq!(
            streaming_output.lines,
            full_output.lines.as_slice(),
            "[{}] char-by-char streaming mismatch for: {:?}",
            mode,
            &text[..text.len().min(50)]
        );
    }

    /// Test both pretty and raw modes.
    #[track_caller]
    fn assert_streaming_matches_full_both(text: &str) {
        assert_streaming_matches_full(text, true);
        assert_streaming_matches_full(text, false);
    }

    /// Stream in variable-sized chunks and compare final output.
    #[track_caller]
    fn assert_streaming_chunks_match_full(text: &str, pretty: bool, chunk_sizes: &[usize]) {
        let mode = if pretty { "pretty" } else { "raw" };
        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, pretty, None);

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, pretty);
        let mut pos = 0;
        let mut chunk_idx = 0;

        while pos < text.len() {
            let desired_end = pos + chunk_sizes[chunk_idx % chunk_sizes.len()];
            // Find next valid char boundary at or after desired_end
            let end = text[pos..]
                .char_indices()
                .map(|(i, _)| pos + i)
                .find(|&i| i >= desired_end)
                .unwrap_or(text.len());
            renderer.push_and_render(&text[pos..end], None);
            pos = end;
            chunk_idx += 1;
        }
        let streaming_output = renderer.view();

        assert_eq!(
            streaming_output.lines,
            full_output.lines.as_slice(),
            "[{}] chunk streaming mismatch for: {:?}",
            mode,
            &text[..text.len().min(50)]
        );
    }

    #[test]
    fn test_comprehensive_char_by_char() {
        assert_streaming_matches_full_both(COMPREHENSIVE_MARKDOWN);
    }

    #[test]
    fn test_comprehensive_small_chunks() {
        assert_streaming_chunks_match_full(COMPREHENSIVE_MARKDOWN, true, &[3, 5, 7, 11]);
        assert_streaming_chunks_match_full(COMPREHENSIVE_MARKDOWN, false, &[3, 5, 7, 11]);
    }

    #[test]
    fn test_comprehensive_large_chunks() {
        assert_streaming_chunks_match_full(COMPREHENSIVE_MARKDOWN, true, &[50, 100, 200]);
        assert_streaming_chunks_match_full(COMPREHENSIVE_MARKDOWN, false, &[50, 100, 200]);
    }

    #[test]
    fn test_individual_block_types() {
        // Headings
        assert_streaming_matches_full_both("# Hello World\n\n");
        assert_streaming_matches_full_both("## Level 2\n\n### Level 3\n\n");

        // Paragraphs
        assert_streaming_matches_full_both("This is a paragraph.\n\n");
        assert_streaming_matches_full_both("Para one.\n\nPara two.\n\n");

        // Code blocks
        assert_streaming_matches_full_both("```rust\nfn main() {}\n```\n");
        assert_streaming_matches_full_both("```\nplain\n```\n");

        // Lists
        assert_streaming_matches_full_both("- Item 1\n- Item 2\n- Item 3\n\n");
        assert_streaming_matches_full_both("1. First\n2. Second\n\n");
        assert_streaming_matches_full_both("- Outer\n  - Inner 1\n  - Inner 2\n\n");

        // Blockquotes
        assert_streaming_matches_full_both("> Quote line 1\n> Quote line 2\n\n");

        // Tables
        assert_streaming_matches_full_both("| A | B |\n|---|---|\n| 1 | 2 |\n\n");

        // Thematic breaks
        assert_streaming_matches_full_both("Above\n\n---\n\nBelow\n\n");
    }

    #[test]
    fn test_mermaid_streaming_matches_full() {
        assert_streaming_matches_full_both(
            "```mermaid\ngraph TD\n  A[Start] --> B{Go?}\n  B -->|yes| C[Ship]\n  B -->|no| A\n```\n\nDone.\n",
        );
    }

    #[test]
    fn test_nested_constructs() {
        // Code in list
        assert_streaming_matches_full_both("- Step 1\n  ```\n  code\n  ```\n- Step 2\n\n");

        // List in blockquote
        assert_streaming_matches_full_both("> Quote:\n> - Item 1\n> - Item 2\n\n");

        // Nested blockquotes
        assert_streaming_matches_full_both("> Outer\n> > Inner\n\n");

        // Deeply nested list
        assert_streaming_matches_full_both("- L1\n  - L2\n    - L3\n\n");
    }

    #[test]
    fn test_inline_formatting() {
        assert_streaming_matches_full_both("Text with **bold**, *italic*, `code`.\n\n");
        assert_streaming_matches_full_both("A [link](url) and ![image](src).\n\n");
        assert_streaming_matches_full_both("***bold italic*** and ~~strike~~.\n\n");
    }

    #[test]
    fn test_edge_cases() {
        // Empty
        assert_streaming_matches_full_both("");

        // Whitespace only
        assert_streaming_matches_full_both("   \n\n  \n");

        // No trailing newline
        assert_streaming_matches_full_both("# Title\n\nNo newline at end");

        // Just a heading (minimal)
        assert_streaming_matches_full_both("# H\n");

        // Multiple blank lines
        assert_streaming_matches_full_both("Para 1\n\n\n\nPara 2\n\n");
    }

    // Line Source Map Correctness Tests

    /// Verify line_source_map matches between streaming and full render.
    /// Uses push_raw() to batch all characters, then update() once at end.
    #[track_caller]
    fn assert_line_source_map_matches(text: &str, pretty: bool) {
        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, pretty, None);

        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, pretty);
        for ch in text.chars() {
            renderer.push(&ch.to_string());
        }
        renderer.render(None);
        let streaming_output = renderer.view();

        // Compare line source maps
        assert_eq!(
            full_output.line_source_map,
            streaming_output.line_source_map,
            "Line source map mismatch for {:?}",
            &text[..text.len().min(50)]
        );
    }

    #[test]
    fn test_line_source_map_simple() {
        // Simple paragraph - no freezing happens
        assert_line_source_map_matches("Hello world.\n\n", true);
    }

    #[test]
    fn test_line_source_map_with_checkpoints() {
        // Multiple blocks with checkpoints
        assert_line_source_map_matches("# Title\n\nParagraph one.\n\nParagraph two.\n\n", true);
    }

    #[test]
    fn test_soft_break_preserved_when_collapse_disabled() {
        // With collapse disabled, soft breaks stay as line breaks so each
        // source line becomes its own rendered line mapping 1:1.
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.set_collapse_soft_breaks(false);
        renderer.push_and_render("Line one,\nLine two,\nLine three.", None);
        let output = renderer.view();

        let texts: Vec<String> = output
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["Line one,", "Line two,", "Line three."]);
        assert_eq!(output.line_source_map, vec![0, 1, 2]);
    }

    #[test]
    fn test_soft_break_collapse_still_default_on() {
        // Default behavior is unchanged: soft breaks collapse to a space.
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.push_and_render("Line one,\nLine two,\nLine three.", None);
        let output = renderer.view();
        let texts: Vec<String> = output
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(texts, vec!["Line one, Line two, Line three."]);
    }

    #[test]
    fn test_soft_break_disabled_preserves_inline_style() {
        // Each preserved line keeps its inline styling (unlike a raw-text
        // fallback). Bold on line 1 must survive.
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        renderer.set_collapse_soft_breaks(false);
        renderer.push_and_render("a **bold** c\nplain line", None);
        let output = renderer.view();
        assert_eq!(output.lines.len(), 2, "lines: {:?}", output.lines);
        let has_bold = output.lines[0].spans.iter().any(|s| {
            s.style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        });
        assert!(has_bold, "bold must survive: {:?}", output.lines[0].spans);
    }

    #[test]
    fn test_streaming_preserves_heading_style() {
        // Verify that streaming produces styled output for headings
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        // Push a complete heading
        renderer.push_and_render("# Heading\n\n", None);
        let output = renderer.view();

        // Should have at least one line
        assert!(!output.lines.is_empty(), "Should produce lines for heading");

        // Check that the first line has styling (heading should be bold and colored)
        let first_line = &output.lines[0];
        assert!(!first_line.spans.is_empty(), "First line should have spans");

        // The heading text should have some style applied (bold, color, etc.)
        let has_style = first_line.spans.iter().any(|span| {
            span.style.fg.is_some()
                || span
                    .style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
        });
        assert!(
            has_style,
            "Heading should have styling (color or bold). Got spans: {:?}",
            first_line.spans
        );
    }

    #[test]
    fn test_incremental_streaming_preserves_styles() {
        // Test that styles are preserved when streaming character by character
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        // Push heading character by character
        for c in "# Heading\n\n".chars() {
            renderer.push_and_render(&c.to_string(), None);
        }
        let output = renderer.view();

        assert!(!output.lines.is_empty(), "Should produce lines");

        // After complete heading, should have styled output
        let first_line = &output.lines[0];
        let has_style = first_line.spans.iter().any(|span| {
            span.style.fg.is_some()
                || span
                    .style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
        });
        assert!(
            has_style,
            "Incrementally streamed heading should have styling. Spans: {:?}",
            first_line.spans
        );
    }

    #[test]
    fn test_line_source_map_code_block() {
        assert_line_source_map_matches("```rust\nlet x = 1;\n```\n", true);
    }

    /// Debug test: trace streaming behavior with demo chunks
    #[test]
    fn test_demo_streaming_chunks() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        let chunks = [
            "# Streaming Demo\n\n",
            "This text is being streamed ",
            "**incrementally** ",
            "just like a real LLM response!\n\n",
        ];

        for (i, chunk) in chunks.iter().enumerate() {
            renderer.push_and_render(chunk, None);
            let output = renderer.view();

            // Collect output info while we still have the borrow
            let line_count = output.lines.len();
            let lines_debug: Vec<String> = output
                .lines
                .iter()
                .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            eprintln!("=== After chunk {} ===", i);
            eprintln!("Chunk: {:?}", chunk);
            eprintln!(
                "Frozen: {} bytes, {} lines",
                renderer.frozen_bytes(),
                renderer.frozen_lines_count()
            );
            eprintln!("Output lines: {}", line_count);
            for (j, text) in lines_debug.iter().enumerate() {
                eprintln!("  Line {}: {:?}", j, text);
            }
            eprintln!();
        }

        // After all chunks, should have 3 lines:
        // - heading
        // - blank line separator (between blocks)
        // - paragraph
        let final_output = renderer.view();
        assert_eq!(
            final_output.lines.len(),
            3,
            "Should have 3 lines: heading + separator + paragraph. Got: {}",
            final_output.lines.len()
        );
    }

    /// Test trailing newlines don't create extra empty lines at the end.
    #[test]
    fn test_no_trailing_empty_lines() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        // Push content ending with blank lines (like the demo table row)
        // The table row "| 10KB | 850ms | 10ms |\n\n" ends with \n\n
        renderer.push_and_render("| A | B |\n|---|---|\n| 1 | 2 |\n\n", None);
        let output = renderer.view();

        let line_count = output.lines.len();
        eprintln!("Total lines: {}", line_count);
        for (i, line) in output.lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            eprintln!("  Line {}: {:?} (empty: {})", i, text, text.is_empty());
        }

        // Check if the last line is empty (which would be a trailing newline issue)
        let last_line = output.lines.last().expect("Should have lines");
        let last_text: String = last_line.spans.iter().map(|s| s.content.as_ref()).collect();

        // The last line should NOT be empty (trailing blank lines are bad)
        assert!(
            !last_text.is_empty(),
            "Last line should not be empty. Got {} lines with last = {:?}",
            line_count,
            last_text
        );
    }
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/streaming/unit/incremental.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/streaming/unit/boundaries.rs"));
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/streaming/unit/regressions.rs"));
