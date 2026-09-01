use super::*;

/// Mouse selection highlight (issue #70): rows in the selection range
/// get the selection background overlaid (fg preserved), rows outside it
/// keep their own styling; the range is in capped coordinates and slides
/// with the scroll offset.

#[test]
fn render_lines_window_highlights_selected_rows() {
    let lines = vec![
        Line::raw("alpha"),
        Line::raw("beta"),
        Line::raw("gamma"),
        Line::styled("delta", Style::new().fg(Color::Red)),
    ];
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 20, 5));
    render_lines_window(
        &mut buf,
        ratatui::layout::Rect::new(0, 0, 20, 5),
        &lines,
        0,
        Some(TextSelection {
            start_line: 1,
            start_col: 0,
            end_line: 2,
            end_col: 20,
        }),
    );
    assert_eq!(buf.cell((0, 0)).unwrap().bg, Color::Reset, "unselected row");
    assert_eq!(buf.cell((0, 1)).unwrap().bg, SELECTION_BG, "first selected row");
    assert_eq!(buf.cell((0, 2)).unwrap().bg, SELECTION_BG, "second selected row");
    assert_eq!(buf.cell((0, 3)).unwrap().bg, Color::Reset, "row after selection");
    assert_eq!(
        buf.cell((0, 3)).unwrap().fg,
        Color::Red,
        "fg survives selection overlay"
    );

    // With a scroll offset the selection stays in capped coordinates.
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 20, 5));
    render_lines_window(
        &mut buf,
        ratatui::layout::Rect::new(0, 0, 20, 5),
        &lines,
        2,
        Some(TextSelection {
            start_line: 3,
            start_col: 0,
            end_line: 3,
            end_col: 20,
        }),
    );
    assert_eq!(
        buf.cell((0, 1)).unwrap().bg,
        SELECTION_BG,
        "capped line 3 = second visible row under offset 2"
    );
    assert_eq!(buf.cell((0, 0)).unwrap().bg, Color::Reset);
}

/// Character-level selection text: slices by display column, joins rows
/// with newlines, and keeps whole wide characters.
#[test]
fn selection_text_slices_characters_by_column() {
    let lines = vec![
        Line::raw("alpha beta"),
        Line::raw("gamma delta"),
        Line::raw("中文测试"),
    ];
    let sel = TextSelection {
        start_line: 0,
        start_col: 6, // after "alpha "
        end_line: 1,
        end_col: 5,  // "gamma"
    };
    assert_eq!(selection_text(&lines, sel), "beta\ngamma");

    // Wide characters are selected whole: start at column 2 (middle of
    // the 2-column "中") selects the full character.
    let sel = TextSelection {
        start_line: 2,
        start_col: 1,
        end_line: 2,
        end_col: 2,
    };
    assert_eq!(selection_text(&lines, sel), "中");
}

/// Character-level highlight only paints the selected columns, not the
/// full row/area.
#[test]
fn render_lines_window_highlights_selected_character_columns() {
    let lines = vec![Line::raw("hello world")];
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 20, 5));
    render_lines_window(
        &mut buf,
        ratatui::layout::Rect::new(0, 0, 20, 5),
        &lines,
        0,
        Some(TextSelection {
            start_line: 0,
            start_col: 6,
            end_line: 0,
            end_col: 11,
        }),
    );
    assert_eq!(buf.cell((6, 0)).unwrap().bg, SELECTION_BG, "first selected col");
    assert_eq!(buf.cell((10, 0)).unwrap().bg, SELECTION_BG, "last selected col");
    assert_eq!(buf.cell((11, 0)).unwrap().bg, Color::Reset, "after end_col");
    assert_eq!(buf.cell((0, 0)).unwrap().bg, Color::Reset, "before start_col");
}
