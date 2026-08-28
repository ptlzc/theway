    use super::*;

    fn plain_text(lines: &[String]) -> String {
        lines.join("\n")
    }

    fn assert_full_timestamp_prefix(row: &str, rendered: &str) {
        assert_eq!(row.chars().nth(4), Some('-'), "{rendered}");
        assert_eq!(row.chars().nth(7), Some('-'), "{rendered}");
        assert_eq!(row.chars().nth(10), Some(' '), "{rendered}");
        assert_eq!(row.chars().nth(13), Some(':'), "{rendered}");
    }

    #[test]
    fn text_deltas_accumulate_into_one_assistant_block() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::TurnStart);
        feed.apply(FeedUpdate::TextDelta(" hello".into()));
        feed.apply(FeedUpdate::TextDelta(" world".into()));
        feed.apply(FeedUpdate::TurnEnd);
        let rendered = plain_text(&feed.plain_lines(80));
        // Leading whitespace before the first visible char is trimmed.
        assert!(rendered.contains("ai ▸ hello world"), "{rendered}");
    }

    #[test]
    fn thinking_then_text_then_tool_keep_separate_blocks() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::TurnStart);
        feed.apply(FeedUpdate::ThinkingDelta("pondering".into()));
        feed.apply(FeedUpdate::TextDelta("answer".into()));
        feed.apply(FeedUpdate::ToolStart {
            name: "read".into(),
            args: "(path=\"x\")".into(),
        });
        feed.apply(FeedUpdate::ToolEnd {
            tool_call_id: "tool-1".into(),
            lines: vec!["line a".into(), "line b".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::TextDelta("after tool".into()));
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("[thinking] pondering"));
        assert!(rendered.contains("answer"));
        assert!(rendered.contains("⚙ read(path=\"x\")"));
        assert!(rendered.contains("    line a"));
        assert!(rendered.contains("after tool"));
        // text-after-tool starts a fresh assistant block, not glued to "answer".
        let idx_answer = rendered.find("answer").unwrap();
        let idx_after = rendered.find("after tool").unwrap();
        assert!(idx_after > idx_answer);
    }

    #[test]
    fn should_separate_only_gaps_user_boundaries_by_default() {
        let user = Block::User {
            text: "hi".into(),
            timestamp: None,
        };
        let assistant = Block::Assistant {
            text: "yo".into(),
            timestamp: None,
        };
        let tool = Block::Tool {
            name: "bash".into(),
            args: "".into(),
            timestamp: None,
        };
        let result = Block::ToolResult {
            tool_call_id: "t".into(),
            lines: vec!["out".into()],
            is_error: false,
            timestamp: None,
        };
        assert!(should_separate(None, &user, true));
        assert!(should_separate(Some(&assistant), &user, true));
        assert!(should_separate(Some(&user), &assistant, true));
        assert!(should_separate(Some(&user), &tool, true));
        // Non-user boundaries are flush by default.
        assert!(!should_separate(Some(&assistant), &tool, true));
        assert!(!should_separate(Some(&tool), &tool, true));
        assert!(!should_separate(Some(&tool), &result, true));
        assert!(!should_separate(Some(&result), &assistant, true));
        // No output yet → never a gap.
        assert!(!should_separate(None, &user, false));
    }

    #[test]
    fn should_separate_with_separate_all_gaps_every_pair() {
        let user = Block::User {
            text: "hi".into(),
            timestamp: None,
        };
        let assistant = Block::Assistant {
            text: "yo".into(),
            timestamp: None,
        };
        let tool = Block::Tool {
            name: "bash".into(),
            args: "".into(),
            timestamp: None,
        };
        assert!(should_separate_with(None, &user, true, true));
        assert!(should_separate_with(Some(&assistant), &tool, true, true));
        assert!(should_separate_with(Some(&tool), &tool, true, true));
        assert!(should_separate_with(Some(&tool), &assistant, true, true));
        // Still never before the first row.
        assert!(!should_separate_with(None, &user, false, true));
        // And the default path is unchanged.
        assert!(!should_separate_with(Some(&tool), &assistant, true, false));
    }

    #[test]
    fn wrap_breaks_on_word_boundaries_and_preserves_indent() {
        let rows = wrap_str("    aaaa bbbb cccc", 10);
        assert_eq!(rows[0], "    aaaa");
        assert!(rows.len() >= 2);
    }

    #[test]
    fn wrap_hard_breaks_overlong_word() {
        let rows = wrap_str("abcdefghij", 4);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn cjk_text_survives_wrapping() {
        let rows = wrap_str("你好世界一二三四", 6);
        // Each CJK glyph is width 2 → 3 per row of width 6.
        assert!(rows.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 6));
        assert_eq!(rows.concat(), "你好世界一二三四");
    }

    #[test]
    fn user_block_gets_prefix_and_blank_separator() {
        let mut feed = Feed::new();
        feed.push_plain("banner", Level::Header);
        feed.push_user("do the thing");
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("you ▸ do the thing"));
        // a blank line separates the banner from the user turn
        assert!(rendered.contains("\n\n"));
    }

    #[test]
    fn user_and_assistant_blocks_have_breathing_room() {
        let mut feed = Feed::new();
        feed.push_user("tight?");
        feed.push_assistant("not anymore");
        let rendered = plain_text(&feed.plain_lines(80));

        assert!(
            rendered.contains("you ▸ tight?\n\n") && rendered.contains("ai ▸ not anymore"),
            "assistant reply should not be glued to the user prompt:\n{rendered}"
        );
    }

    #[test]
    fn user_and_tool_first_reply_have_breathing_room() {
        let mut feed = Feed::new();
        feed.push_user("inspect");
        feed.push_tool("read", "(path=\"x\")");
        feed.push_tool_result("tool-1", vec!["contents".into()], false);
        let rendered = plain_text(&feed.plain_lines(80));

        assert!(
            rendered.contains("you ▸ inspect\n\n")
                && rendered.contains("⚙ read(path=\"x\")")
                && rendered.contains("    contents"),
            "tool-first assistant activity should not be glued to the user prompt, but tool result should stay with the tool call:\n{rendered}"
        );
    }

    #[test]
    fn rendered_message_blocks_include_short_time_prefix() {
        let mut feed = Feed::new();
        feed.push_user("hello");
        feed.push_assistant("hi");
        feed.push_tool("read", "(path=\"x\")");
        feed.push_tool_result("tool-1", vec!["ok".into()], false);
        let rendered = plain_text(&feed.plain_lines(120));
        let rows: Vec<&str> = rendered.lines().collect();

        assert!(rows[0].contains("you ▸ hello"), "{rendered}");
        assert_full_timestamp_prefix(rows[0], &rendered);
        assert!(rows[2].contains("ai ▸ hi"), "{rendered}");
        assert_full_timestamp_prefix(rows[2], &rendered);
        assert!(rows[3].contains("⚙ read(path=\"x\")"), "{rendered}");
        assert_full_timestamp_prefix(rows[3], &rendered);
        assert!(rows[4].contains("    ok"), "{rendered}");
        assert_full_timestamp_prefix(rows[4], &rendered);
    }

    #[test]
    fn timestamp_label_includes_full_date_and_time() {
        let today = Local
            .with_ymd_and_hms(2026, 5, 27, 14, 37, 0)
            .single()
            .unwrap();
        let same_day = today.with_timezone(&Utc);
        let previous_day = Local
            .with_ymd_and_hms(2026, 5, 26, 23, 59, 0)
            .single()
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(format_timestamp_label(same_day, today), "2026-05-27 14:37");
        assert_eq!(
            format_timestamp_label(previous_day, today),
            "2026-05-26 23:59"
        );
    }

    #[test]
    fn narrow_width_keeps_timestamped_blocks_renderable() {
        let mut feed = Feed::new();
        feed.push_user("a very long message that wraps");
        let rendered = plain_text(&feed.plain_lines(16));

        assert!(rendered.contains("you ▸"), "{rendered}");
        assert!(
            rendered
                .lines()
                .all(|line| UnicodeWidthStr::width(line) <= 16),
            "{rendered}"
        );
    }

    #[test]
    fn compact_tool_output_keeps_short_output_unchanged() {
        let lines = vec!["ok".to_string(), "done".to_string()];
        assert_eq!(compact_tool_output_lines(lines.clone(), false), lines);
    }

    #[test]
    fn compact_tool_output_keeps_head_and_tail_with_summary() {
        let lines: Vec<String> = (0..40).map(|i| format!("line {i}")).collect();
        let compacted = compact_tool_output_lines(lines, false);

        assert!(compacted.len() <= TOOL_OUTPUT_HEAD_LINES + TOOL_OUTPUT_TAIL_LINES + 1);
        assert_eq!(compacted.first().map(String::as_str), Some("line 0"));
        assert!(compacted.iter().any(|line| line.contains("truncated")));
        assert!(
            compacted
                .iter()
                .any(|line| line.contains("full output remains available to the agent"))
        );
        assert_eq!(compacted.last().map(String::as_str), Some("line 39"));
    }

    #[test]
    fn compact_tool_output_allows_more_error_context() {
        let lines: Vec<String> = (0..36).map(|i| format!("line {i}")).collect();

        assert!(
            compact_tool_output_lines(lines.clone(), false)
                .iter()
                .any(|line| line.contains("truncated"))
        );
        assert_eq!(compact_tool_output_lines(lines, true).len(), 36);
    }

    #[test]
    fn compact_tool_output_truncates_utf8_safely() {
        let long = "你好".repeat(TOOL_OUTPUT_MAX_LINE_CHARS + 10);
        let compacted = compact_tool_output_lines(vec![long], false);

        assert!(compacted[0].ends_with('…'));
        assert!(compacted.iter().any(|line| line.contains("truncated")));
    }

    #[test]
    fn last_thinking_block_finds_trailing_thinking() {
        let mut feed = Feed::new();
        feed.push_user("go");
        feed.apply(FeedUpdate::ThinkingDelta("pondering".into()));
        feed.apply(FeedUpdate::ThinkingDelta(" more".into()));
        feed.apply(FeedUpdate::ToolStart {
            name: "read".into(),
            args: "(path=\"x\")".into(),
        });
        let (index, text) = feed.last_thinking_block().unwrap();
        assert_eq!(index, 1);
        assert_eq!(text, "pondering more");
        // Tool block after thinking does not shadow it.
        assert_eq!(feed.last_thinking_block().map(|(i, _)| i), Some(1));
    }

    #[test]
    fn set_thinking_block_replaces_only_thinking() {
        let mut feed = Feed::new();
        feed.push_user("go");
        feed.apply(FeedUpdate::ThinkingDelta("raw thoughts".into()));
        assert!(feed.set_thinking_block(1, "structured summary".into()));
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("structured summary"));
        assert!(!rendered.contains("raw thoughts"));
        // Non-thinking block and out-of-range indices are rejected.
        assert!(!feed.set_thinking_block(0, "nope".into()));
        assert!(!feed.set_thinking_block(99, "nope".into()));
    }

    #[test]
    fn thinking_summary_update_backfills_block() {
        let mut feed = Feed::new();
        feed.push_user("go");
        feed.apply(FeedUpdate::ThinkingDelta("verbose".into()));
        feed.apply(FeedUpdate::ThinkingSummary {
            block_index: 1,
            summary: "## Goal\n- do it".into(),
        });
        let rendered = plain_text(&feed.plain_lines(80));
        assert!(rendered.contains("## Goal"));
        assert!(!rendered.contains("verbose"));
    }

    #[test]
    fn tool_progress_for_same_call_is_replaced_not_appended() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["old progress".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["new progress".into()],
            is_error: false,
        });

        let rendered = plain_text(&feed.plain_lines(80));
        assert!(!rendered.contains("old progress"));
        assert!(rendered.contains("new progress"));
    }

    #[test]
    fn final_tool_output_replaces_progress_for_same_call() {
        let mut feed = Feed::new();
        feed.apply(FeedUpdate::ToolProgress {
            tool_call_id: "tool-1".into(),
            lines: vec!["progress".into()],
            is_error: false,
        });
        feed.apply(FeedUpdate::ToolEnd {
            tool_call_id: "tool-1".into(),
            lines: vec!["final result".into()],
            is_error: false,
        });

        let rendered = plain_text(&feed.plain_lines(80));
        assert!(!rendered.contains("progress"));
        assert!(rendered.contains("final result"));
    }

    #[test]
    fn replace_block_updates_matching_kind_and_rejects_gaps() {
        let mut feed = Feed::new();
        feed.replace_blocks(&[
            WireFeedBlock::Thinking {
                text: "old".into(),
                timestamp: Some("09:00".into()),
            },
            WireFeedBlock::ToolResult {
                lines: vec!["progress".into()],
                is_error: false,
                timestamp: None,
            },
        ]);

        assert!(feed.replace_block(
            0,
            &WireFeedBlock::Thinking {
                text: "summary".into(),
                timestamp: Some("09:01".into()),
            }
        ));
        assert!(feed.replace_block(
            1,
            &WireFeedBlock::ToolResult {
                lines: vec!["done".into()],
                is_error: true,
                timestamp: Some("09:02".into()),
            }
        ));
        assert!(!feed.replace_block(
            0,
            &WireFeedBlock::Assistant {
                text: "wrong kind".into(),
                timestamp: None,
            }
        ));
        assert!(!feed.replace_block(
            2,
            &WireFeedBlock::Thinking {
                text: "gap".into(),
                timestamp: None,
            }
        ));

        let blocks = feed.blocks();
        assert!(matches!(
            &blocks[0],
            Block::Thinking { text, timestamp }
                if text == "summary" && timestamp.as_deref() == Some("09:00")
        ));
        assert!(matches!(
            &blocks[1],
            Block::ToolResult { tool_call_id, lines, is_error, timestamp }
                if tool_call_id.is_empty()
                    && lines == &["done"]
                    && *is_error
                    && timestamp.is_none()
        ));
    }