    /// Test the actual demo ending chunks don't produce trailing empty lines.
    #[test]
    fn test_demo_ending_no_trailing_lines() {
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);

        // These are the actual final chunks from the demo
        let chunks = [
            "| Feature | Before | After |\n",
            "|---------|--------|-------|\n",
            "| Complexity | O(N²) | O(N) |\n",
            "| 10KB render | 850ms | 10ms |\n\n",
            "✨ *Streaming complete!*",
        ];

        for chunk in chunks {
            renderer.push_and_render(chunk, None);
        }
        let output = renderer.view();

        let line_count = output.lines.len();
        eprintln!("Total lines after demo ending: {}", line_count);
        for (i, line) in output.lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            eprintln!("  Line {}: {:?}", i, text);
        }

        // Count trailing empty lines
        let trailing_empty = output
            .lines
            .iter()
            .rev()
            .take_while(|line| {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text.is_empty()
            })
            .count();

        assert_eq!(
            trailing_empty, 0,
            "Should have no trailing empty lines, got {}",
            trailing_empty
        );
    }

    /// Test that streaming produces the same output as full render.
    /// This is the key correctness test - streaming should be identical to full render.
    #[test]
    fn test_streaming_matches_full_render_with_block_spacing() {
        let full_content = "# Heading\n\nParagraph one.\n\n## Subheading\n\nParagraph two.\n\n";

        // Full render
        let (full_output, _) =
            render_markdown_ratatui_full(full_content, test_style::STYLE, true, None);

        // Streaming render (chunk by chunk)
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        let chunks = [
            "# Heading\n\n",
            "Paragraph one.\n\n",
            "## Subheading\n\n",
            "Paragraph two.\n\n",
        ];
        for chunk in chunks {
            renderer.push_and_render(chunk, None);
            // push() now renders automatically // Render after each chunk
        }
        let streaming_output = renderer.view();

        // Debug output
        eprintln!("Full render ({} lines):", full_output.lines.len());
        for (i, line) in full_output.lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            eprintln!("  Line {}: {:?}", i, text);
        }
        eprintln!("Streaming render ({} lines):", streaming_output.lines.len());
        for (i, line) in streaming_output.lines.iter().enumerate() {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            eprintln!("  Line {}: {:?}", i, text);
        }

        // Also check what each chunk produces individually
        eprintln!("\nIndividual chunk renders:");
        for chunk in &chunks {
            let (chunk_output, _) =
                render_markdown_ratatui_full(chunk, test_style::STYLE, true, None);
            eprintln!("Chunk {:?} -> {} lines:", chunk, chunk_output.lines.len());
            for (i, line) in chunk_output.lines.iter().enumerate() {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                eprintln!("    Line {}: {:?}", i, text);
            }
        }

        // They should match exactly
        assert_eq!(
            streaming_output.lines.len(),
            full_output.lines.len(),
            "Streaming should produce same number of lines as full render"
        );

        for (i, (stream_line, full_line)) in streaming_output
            .lines
            .iter()
            .zip(full_output.lines.iter())
            .enumerate()
        {
            let stream_text: String = stream_line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let full_text: String = full_line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(
                stream_text, full_text,
                "Line {} mismatch: streaming={:?}, full={:?}",
                i, stream_text, full_text
            );
        }
    }

    // Comprehensive blank line tests - streaming must match full render exactly

    /// Helper to compare streaming vs full render for given chunks.
    /// Content is derived by joining chunks - no need to specify it separately.
    #[track_caller]
    fn assert_streaming_equals_full(chunks: &[&str], description: &str) {
        // Derive content from chunks
        let content: String = chunks.iter().copied().collect();

        // Full render
        let (full_output, _) =
            render_markdown_ratatui_full(&content, test_style::STYLE, true, None);

        // Streaming render
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in chunks {
            renderer.push_and_render(chunk, None);
            // push() now renders automatically
        }
        let streaming_output = renderer.view();

        // Collect info while we have the borrow
        let streaming_lines: Vec<String> = streaming_output
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let frozen_bytes = renderer.frozen_bytes();
        let frozen_source = renderer.source()[..frozen_bytes].to_string();
        let trailing_blanks = count_trailing_blank_lines(&frozen_source);

        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        // Check for mismatch
        let lines_match = streaming_lines.len() == full_lines.len();
        let content_matches = streaming_lines == full_lines;

        if !lines_match || !content_matches {
            eprintln!("=== {} ===", description);
            eprintln!("Content: {:?}", content);
            eprintln!("Chunks: {:?}", chunks);
            eprintln!("Full render ({} lines):", full_lines.len());
            for (i, text) in full_lines.iter().enumerate() {
                eprintln!("  Line {}: {:?}", i, text);
            }
            eprintln!("Streaming render ({} lines):", streaming_lines.len());
            for (i, text) in streaming_lines.iter().enumerate() {
                eprintln!("  Line {}: {:?}", i, text);
            }
            eprintln!(
                "Frozen source ({} bytes): {:?}",
                frozen_bytes, frozen_source
            );
            eprintln!("Trailing blank lines in frozen: {}", trailing_blanks);
        }

        assert_eq!(
            streaming_lines.len(),
            full_lines.len(),
            "{}: line count mismatch",
            description
        );

        for (i, (stream_text, full_text)) in
            streaming_lines.iter().zip(full_lines.iter()).enumerate()
        {
            assert_eq!(
                stream_text, full_text,
                "{}: line {} mismatch",
                description, i
            );
        }
    }

    #[test]
    fn test_streaming_double_newline() {
        assert_streaming_equals_full(&["# Heading\n\n", "Paragraph\n\n"], "double newline");
    }

    #[test]
    fn test_streaming_triple_newline() {
        assert_streaming_equals_full(&["# Heading\n\n\n", "Paragraph\n\n"], "triple newline");
    }

    #[test]
    fn test_streaming_quadruple_newline() {
        assert_streaming_equals_full(&["# Heading\n\n\n\n", "Paragraph\n\n"], "quadruple newline");
    }

    #[test]
    fn test_streaming_newlines_with_spaces() {
        assert_streaming_equals_full(
            &["# Heading\n\n  \n\n", "Paragraph\n\n"],
            "newlines with spaces",
        );
    }

    #[test]
    fn test_streaming_code_block_then_paragraph() {
        // Test that blank line after code block is preserved
        assert_streaming_equals_full(
            &[
                "```rust\nfn main() {}\n```\n\n",
                "Paragraph after code.\n\n",
            ],
            "code block then paragraph",
        );
    }

    /// Test that code blocks have proper syntax highlighting in streaming mode.
    #[test]
    fn test_streaming_code_block_syntax_highlighting() {
        let chunks = [
            "```rust\n",
            "fn main() {\n",
            "    println!(\"Hello!\");\n",
            "}\n",
            "```\n\n",
        ];
        let content: String = chunks.iter().copied().collect();

        // Full render
        let (full_output, _) =
            render_markdown_ratatui_full(&content, test_style::STYLE, true, None);

        // Streaming render
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in &chunks {
            renderer.push_and_render(chunk, None);
            // push() now renders automatically
        }
        let streaming_output = renderer.view();

        // Check that streaming and full have the same line count
        assert_eq!(
            streaming_output.lines.len(),
            full_output.lines.len(),
            "Line count should match"
        );

        // Check that spans match (indicating syntax highlighting worked)
        for (i, (stream_line, full_line)) in streaming_output
            .lines
            .iter()
            .zip(full_output.lines.iter())
            .enumerate()
        {
            assert_eq!(
                stream_line.spans.len(),
                full_line.spans.len(),
                "Line {} span count should match (syntax highlighting)",
                i
            );
        }
    }

    #[test]
    fn test_streaming_code_block_triple_newline() {
        // Test multiple blank lines after code block
        assert_streaming_equals_full(
            &["```rust\nfn main() {}\n```\n\n\n", "Paragraph\n\n"],
            "code block triple newline",
        );
    }

    #[test]
    fn test_streaming_table_then_paragraph() {
        assert_streaming_equals_full(
            &["| A | B |\n|---|---|\n| 1 | 2 |\n\n", "Paragraph\n\n"],
            "table then paragraph",
        );
    }

    #[test]
    fn test_streaming_list_then_paragraph() {
        assert_streaming_equals_full(
            &["- Item 1\n- Item 2\n\n", "Paragraph\n\n"],
            "list then paragraph",
        );
    }

    #[test]
    fn test_streaming_blockquote_then_paragraph() {
        assert_streaming_equals_full(
            &["> Quote line 1\n> Quote line 2\n\n", "Paragraph\n\n"],
            "blockquote then paragraph",
        );
    }

    #[test]
    fn test_streaming_nested_blockquote() {
        // Test nested blockquotes with "│ │ " prefix
        assert_streaming_equals_full(
            &[
                "> Outer quote\n>> Nested quote\n> Back to outer\n\n",
                "Paragraph\n\n",
            ],
            "nested blockquote",
        );
    }

    /// Nested blockquote with blank lines and a list, streamed token-by-token.
    #[test]
    fn test_streaming_nested_blockquote_with_list() {
        // Token-by-token (realistic LLM chunking)
        assert_streaming_equals_full(
            &["> Foo\n", ">\n", "> > Bar\n", "> >\n", "> > - Baz\n"],
            "nested blockquote with list (line-by-line)",
        );
        // Also test char-by-char
        assert_streaming_matches_full("> Foo\n>\n> > Bar\n> >\n> > - Baz\n", true);
    }

    #[test]
    fn test_blockquote_prefix_rendering() {
        // Verify blockquotes render with "│" prefix instead of ">"
        let text = "> Single line quote\n\n>> Nested quote\n\n";
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);

        let lines: Vec<String> = output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(lines[0], "│ Single line quote");
        assert_eq!(lines[2], "││ Nested quote");
    }

    #[test]
    fn test_streaming_thematic_break() {
        assert_streaming_equals_full(&["Above\n\n", "---\n\n", "Below\n\n"], "thematic break");
    }

    /// Regression: thematic break `---` at the end of a chunk (no trailing newline)
    /// was invisible in pretty mode because the checkpoint's `output_lines` didn't
    /// include the `───` line (it was pending in `current_spans`, unflushed).
    ///
    #[test]
    fn test_streaming_thematic_break_at_chunk_boundary() {
        assert_streaming_equals_full(
            &["hello\n\n---", "\n### World", "\nText"],
            "thematic break at chunk boundary",
        );
    }

    #[test]
    fn test_streaming_multiple_paragraphs() {
        assert_streaming_equals_full(
            &["Para 1\n\n", "Para 2\n\n", "Para 3\n\n"],
            "multiple paragraphs",
        );
    }

    #[test]
    fn test_streaming_mixed_blocks() {
        assert_streaming_equals_full(
            &[
                "# Heading\n\n",
                "Paragraph.\n\n",
                "```\ncode\n```\n\n",
                "- list\n\n",
                "> quote\n\n",
            ],
            "mixed blocks",
        );
    }

    /// The exact content from pager_v3_demo that shows bugs.
    const DEMO_CONTENT: &str = r#"# Streaming Demo

This text is being streamed **incrementally** just like a real LLM response!


## How It Works

The `StreamingMarkdownRenderer` efficiently handles chunks by:

1. Accumulating text in a buffer
2. Detecting stable block boundaries
3. Freezing rendered output up to checkpoints
4. Only re-rendering the tail

```rust
// This code block appears character by character!
fn stream_demo() {
    println!("Hello from streaming!");
}
```

The frozen lines are **never re-rendered**, making streaming O(N) instead of O(N²).

> **Note:** This blockquote contains *italic*, **bold**, and `inline code`.
> It spans multiple lines to test blockquote rendering.

---

| Feature | Before | After |
|---------|--------|-------|
| Complexity | O(N²) | O(N) |
| 10KB render | 850ms | 10ms |

✨ *Streaming complete!*"#;

    /// Test streaming with 4-char chunks (matches demo)
    #[test]
    fn test_demo_content_4char_chunks() {
        // Split into 4-char chunks like the demo
        let chunks = split_into_chunks(DEMO_CONTENT, 4);
        let full_content: String = chunks.iter().copied().collect();
        assert_eq!(full_content, DEMO_CONTENT);

        // Full render
        let (full_output, _) =
            render_markdown_ratatui_full(DEMO_CONTENT, test_style::STYLE, true, None);

        // Streaming render
        let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
        for chunk in &chunks {
            renderer.push_and_render(chunk, None);
            // push() now renders automatically
        }
        let streaming_output = renderer.view();

        // Compare line by line
        assert_eq!(
            streaming_output.lines.len(),
            full_output.lines.len(),
            "Demo content: Line count should match (streaming: {}, full: {})",
            streaming_output.lines.len(),
            full_output.lines.len()
        );

        for (i, (stream_line, full_line)) in streaming_output
            .lines
            .iter()
            .zip(full_output.lines.iter())
            .enumerate()
        {
            let stream_text: String = stream_line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            let full_text: String = full_line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(
                stream_text, full_text,
                "Demo content line {}: text mismatch\nStreaming: {:?}\nFull: {:?}",
                i, stream_text, full_text
            );
        }
    }

    /// Comprehensive test document with various edge cases.
    /// Covers: headings, ALL list types, blockquotes, code blocks, styling,
    /// various newline patterns (double, triple, quadruple),
    /// spaces/tabs between newlines, etc.
    const EDGE_CASE_DOC: &str = concat!(
        // Heading with double newline (standard)
        "# Heading One\n\n",
        // Paragraph
        "Some **bold** and *italic* text.\n\n",
        // Heading with triple newline
        "## Heading Two\n\n\n",
        // Numbered list (1. 2. 3.)
        "1. First item\n",
        "2. Second item\n",
        "3. Third item\n\n",
        // Heading with quadruple newline
        "### Heading Three\n\n\n\n",
        // Blockquote with styling
        "> Quote with **bold** and `code`\n",
        "> Second quote line\n\n",
        // Heading with space between newlines
        "#### Heading Four\n \n",
        // Dash bullet list (-)
        "- Dash one\n",
        "- Dash two\n\n",
        // Asterisk bullet list (*)
        "* Star one\n",
        "* Star two\n\n",
        // Plus bullet list (+)
        "+ Plus one\n",
        "+ Plus two\n\n",
        // Nested list
        "- Parent item\n",
        "  - Nested child\n",
        "  - Another child\n",
        "- Back to parent\n\n",
        // Heading with tab between newlines
        "##### Heading Five\n\t\n",
        // Code block
        "```rust\nfn main() {\n    println!(\"Hello\");\n}\n```\n\n",
        // Heading
        "###### Heading Six\n\n",
        // RESTORE: Mixed whitespace - space, newline, tab, newline (the hidden bug!)
        "Final paragraph.\n \n\t\n",
        // Trailing content
        "The end.",
    );

    /// Test ALL possible 2-way split points for the edge case document.
    #[test]
    fn test_edge_cases_2way_splits() {
        let text = EDGE_CASE_DOC;

        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        let mut failures = Vec::new();

        // Test every possible split point
        for split_at in 1..text.len() {
            if !text.is_char_boundary(split_at) {
                continue;
            }

            let chunk1 = &text[..split_at];
            let chunk2 = &text[split_at..];

            let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
            renderer.push_and_render(chunk1, None);
            // push() now renders automatically
            renderer.push_and_render(chunk2, None);
            let streaming_output = renderer.view();

            let streaming_lines: Vec<String> = streaming_output
                .lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect();

            if streaming_lines != full_lines {
                // Find first difference
                let diff_line = streaming_lines
                    .iter()
                    .zip(full_lines.iter())
                    .enumerate()
                    .find(|(_, (s, f))| s != f)
                    .map(|(i, _)| i);

                failures.push(format!(
                    "byte {}: stream={} lines, full={} lines, first_diff={:?}\n  chunk1_end: {:?}\n  chunk2_start: {:?}",
                    split_at,
                    streaming_lines.len(),
                    full_lines.len(),
                    diff_line,
                    &chunk1[chunk1.len().saturating_sub(30)..],
                    &chunk2[..chunk2.len().min(30)],
                ));
            }
        }

        if !failures.is_empty() {
            panic!(
                "{} failures out of {} split points:\n{}",
                failures.len(),
                text.len() - 1,
                failures.join("\n")
            );
        }
    }

    /// Test 4-way splits: split into 2, then split each half again.
    /// This catches bugs that only manifest with multiple re-renders.
    #[test]
    fn test_edge_cases_4way_splits() {
        let text = EDGE_CASE_DOC;

        let (full_output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        let full_lines: Vec<String> = full_output
            .lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        let mut failures = Vec::new();
        let mut tested = 0;

        // For each primary split point
        for split1 in 1..text.len() {
            if !text.is_char_boundary(split1) {
                continue;
            }

            let first_half = &text[..split1];
            let second_half = &text[split1..];

            // Split first half (if possible)
            let first_splits: Vec<usize> = if first_half.len() > 1 {
                vec![first_half.len() / 2]
            } else {
                vec![first_half.len()] // No split, use whole thing
            };

            // Split second half (if possible)
            let second_splits: Vec<usize> = if second_half.len() > 1 {
                vec![second_half.len() / 2]
            } else {
                vec![second_half.len()]
            };

            for &sub1 in &first_splits {
                // Ensure valid char boundary
                let sub1 = find_char_boundary(first_half, sub1);

                for &sub2 in &second_splits {
                    let sub2 = find_char_boundary(second_half, sub2);

                    let chunks: Vec<&str> = vec![
                        &first_half[..sub1],
                        &first_half[sub1..],
                        &second_half[..sub2],
                        &second_half[sub2..],
                    ]
                    .into_iter()
                    .filter(|c| !c.is_empty())
                    .collect();

                    if chunks.len() < 2 {
                        continue; // Need at least 2 chunks
                    }

                    tested += 1;

                    let mut renderer = StreamingMarkdownRenderer::new(test_style::STYLE, true);
                    for chunk in &chunks {
                        renderer.push_and_render(chunk, None);
                        // push() now renders automatically
                    }
                    let streaming_output = renderer.view();

                    let streaming_lines: Vec<String> = streaming_output
                        .lines
                        .iter()
                        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                        .collect();

                    if streaming_lines != full_lines {
                        let chunk_preview: Vec<String> = chunks
                            .iter()
                            .map(|c| {
                                if c.len() > 15 {
                                    format!("{:?}...", &c[..15])
                                } else {
                                    format!("{:?}", c)
                                }
                            })
                            .collect();
                        failures.push(format!(
                            "4-way split at {}: [{}]",
                            split1,
                            chunk_preview.join(", ")
                        ));
                    }
                }
            }
        }

        if !failures.is_empty() {
            panic!(
                "{} failures out of {} 4-way split combinations:\n{}",
                failures.len(),
                tested,
                failures
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
    }

    /// Find nearest valid char boundary at or before `pos`.
    fn find_char_boundary(s: &str, pos: usize) -> usize {
        let mut p = pos.min(s.len());
        while p > 0 && !s.is_char_boundary(p) {
            p -= 1;
        }
        p
    }
