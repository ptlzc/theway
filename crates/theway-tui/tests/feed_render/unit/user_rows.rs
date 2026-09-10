use super::*;
use theway_transport::feed::{WireFeedAttachment, WireFeedSource};

/// Rendered row text with trailing band padding trimmed.
fn row(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn user_with(
    text: &str,
    attachments: Vec<WireFeedAttachment>,
    source: Option<WireFeedSource>,
) -> WireFeedBlock {
    WireFeedBlock::User {
        text: text.into(),
        timestamp: None,
        attachments,
        source,
    }
}

fn attachment(kind: &str, name: &str, detail: Option<&str>) -> WireFeedAttachment {
    WireFeedAttachment {
        kind: kind.into(),
        name: name.into(),
        detail: detail.map(str::to_string),
    }
}

/// Every attachment renders one muted chip row after the user text rows,
/// aligned with the user block's continuation indent; `detail` appends as a
/// ` · <detail>` suffix and the block text itself stays untouched.
#[test]
fn user_block_renders_one_chip_row_per_attachment() {
    let feed = feed_with(&[user_with(
        "look at these",
        vec![
            attachment("file", "foo.rs", Some("src/foo.rs")),
            attachment("image", "diagram", None),
        ],
        None,
    )]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());

    assert_eq!(lines.len(), 3, "text row + two chip rows: {lines:?}");
    assert_eq!(row(&lines[0]), "\u{276f} look at these");
    assert_eq!(row(&lines[1]), "  [file] foo.rs · src/foo.rs");
    assert_eq!(row(&lines[2]), "  [image] diagram");
    // Muted metadata role, no user band background.
    for chip in &lines[1..] {
        for span in &chip.spans {
            assert_eq!(span.style.fg, Some(TOOL_ARGS_DEFAULT));
            assert_eq!(span.style.bg, None);
        }
    }
    // Chips never leak into the user's text row.
    assert!(!row(&lines[0]).contains("foo.rs"), "{:?}", row(&lines[0]));
}

/// A chip-less user block renders exactly its text rows.
#[test]
fn user_block_without_attachments_renders_text_only() {
    let feed = feed_with(&[user_with("plain", Vec::new(), None)]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    assert_eq!(lines.len(), 1);
    assert_eq!(row(&lines[0]), "\u{276f} plain");
}

/// A user turn whose `source.kind` is not `"user"` is prefixed with a muted
/// `[<kind>: <label>]` provenance marker, so it never reads as a human
/// message. `source.kind == "user"` and a missing source render no marker.
#[test]
fn non_user_source_prepends_provenance_marker() {
    let feed = feed_with(&[user_with(
        "run the nightly check",
        Vec::new(),
        Some(WireFeedSource {
            kind: "trigger".into(),
            label: Some("tr-1".into()),
        }),
    )]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    assert_eq!(lines.len(), 2, "marker row + user row: {lines:?}");
    assert_eq!(row(&lines[0]), "  [trigger: tr-1]");
    assert_eq!(row(&lines[1]), "\u{276f} run the nightly check");
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_ARGS_DEFAULT));
    assert_eq!(lines[0].spans[0].style.bg, None);

    // A producer with no label keeps the kind-only marker.
    let feed = feed_with(&[user_with(
        "delegated question",
        Vec::new(),
        Some(WireFeedSource {
            kind: "subagent".into(),
            label: None,
        }),
    )]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    assert_eq!(row(&lines[0]), "  [subagent]");
    assert_eq!(row(&lines[1]), "\u{276f} delegated question");

    // A human turn carries no provenance marker.
    let feed = feed_with(&[user_with(
        "typed by a human",
        Vec::new(),
        Some(WireFeedSource {
            kind: "user".into(),
            label: None,
        }),
    )]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(row(&lines[0]), "\u{276f} typed by a human");
}

/// A source-less user block (legacy message without a structured record)
/// renders no provenance marker.
#[test]
fn missing_source_renders_no_provenance_marker() {
    let feed = feed_with(&[user_with("legacy turn", Vec::new(), None)]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(row(&lines[0]), "\u{276f} legacy turn");
}

/// A `Context` block renders its own muted `[<label>] <text>` row, with the
/// wrapped continuation rows indented under the text column, and no user
/// band or `❯` prefix. The multi-line body is flattened into one logical line
/// by the shared `context_line` projection, so the plain-line cache and this
/// styled row agree.
#[test]
fn context_block_renders_label_row_with_indented_continuation() {
    let feed = feed_with(&[WireFeedBlock::Context {
        label: "skill:git".into(),
        text: "one two three four five six\nsecond paragraph".into(),
        timestamp: None,
    }]);
    let lines = super::lines(&feed, 24, &FeedRenderOptions::default());

    let rows: Vec<String> = lines.iter().map(row).collect();
    assert_eq!(
        rows,
        vec![
            "[skill:git] one two",
            "            three four",
            "            five six",
            "            second",
            "            paragraph",
        ],
        "context rows: {rows:?}"
    );
    // The label prefix is 12 columns; continuation rows align under the body.
    for line in &lines {
        for span in &line.spans {
            assert_eq!(span.style.fg, Some(THINKING_TEXT_DEFAULT));
            assert_eq!(span.style.bg, None);
        }
    }
}

/// A single-line context block that fits the width renders one row, and the
/// label-less form renders the bare body.
#[test]
fn context_block_single_line_and_bare_label() {
    let feed = feed_with(&[
        WireFeedBlock::Context {
            label: "trigger".into(),
            text: "patch applied".into(),
            timestamp: None,
        },
        WireFeedBlock::Context {
            label: String::new(),
            text: "extension note".into(),
            timestamp: None,
        },
    ]);
    let lines = super::lines(&feed, 40, &FeedRenderOptions::default());
    let rows: Vec<String> = lines.iter().map(row).collect();
    // Context rows are not user boundaries: no gap between them.
    assert_eq!(rows, vec!["[trigger] patch applied", "extension note"]);
}
