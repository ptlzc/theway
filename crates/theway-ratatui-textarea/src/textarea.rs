use crate::editor::{
    ApplyEditPlanError, EditBuffer, EditCommand, EditCommandCategory, EditOutcome, EditPlan,
    WordStyle, classify_key_event,
};
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidgetRef;
use ratatui::widgets::WidgetRef;
use ratatui_core::buffer::Buffer as CoreBuffer;
use ratatui_core::layout::Rect as CoreRect;
use ratatui_core::widgets::Widget as _;
use std::cell::Ref;
use std::cell::RefCell;
use std::ops::Range;
use std::time::Instant;
use textwrap::Options;
use tui_scrollbar::{ScrollBar, ScrollLengths};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Stable, unique identifier for a text element. Monotonically increasing, never reused.
///
/// The host app can use this as a key into its own metadata store
/// (e.g. `HashMap<ElementId, PasteMetadata>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementId(u64);

impl ElementId {
    /// Construct an `ElementId` from a raw `u64` value.
    ///
    /// Primarily useful for tests and serialization; normal code should use
    /// the IDs returned by [`TextArea::insert_element`].
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// Opaque element kind tag. The textarea does not interpret this value;
/// the host app defines constants like `ElementKind(1)` for pastes,
/// `ElementKind(2)` for file references, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElementKind(pub u16);

// ── Clipboard ──

/// Trait for clipboard access. The textarea calls this on copy/cut/paste.
///
/// The default implementation ([`InternalClipboard`]) stores text in memory.
/// Host apps can provide a system clipboard backend (e.g. `arboard`) via
/// [`TextArea::set_clipboard_provider`].
pub trait ClipboardProvider: std::fmt::Debug {
    /// Read the current clipboard contents (for paste).
    fn get(&mut self) -> Option<String>;
    /// Write text to the clipboard (on copy/cut).
    fn set(&mut self, text: &str);
}

/// In-memory clipboard — the default provider.
#[derive(Debug, Default)]
pub struct InternalClipboard {
    contents: Option<String>,
}

impl ClipboardProvider for InternalClipboard {
    fn get(&mut self) -> Option<String> {
        self.contents.clone()
    }

    fn set(&mut self, text: &str) {
        self.contents = Some(text.to_string());
    }
}

// ── Text element events ──

/// An interaction with a [`TextElement`], returned by [`TextArea::poll_element_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextElementEvent {
    /// The element that was interacted with.
    pub id: ElementId,
    /// What kind of interaction occurred.
    pub kind: TextElementEventKind,
}

/// The kind of element interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextElementEventKind {
    /// The element was clicked (single click).
    Click,
    /// The mouse entered the element (was outside or on a different element).
    HoverEnter,
    /// The mouse left the element (moved to plain text or a different element).
    HoverLeave,
}

/// An atomic text element embedded in the buffer.
///
/// Elements are indivisible units for navigation and editing. The cursor
/// cannot be placed inside an element; it jumps from the start boundary
/// to the end boundary atomically.
#[derive(Debug, Clone)]
pub struct TextElement {
    /// Stable identifier, unique across the lifetime of the `TextArea`.
    pub id: ElementId,
    /// Byte range in the underlying text buffer.
    pub range: Range<usize>,
    /// Host-defined kind tag.
    pub kind: ElementKind,
    /// Custom display text and styling. When `Some`, this `Line` is rendered
    /// instead of the raw buffer text. When `None`, the buffer text is rendered
    /// with a default element style (cyan).
    pub display: Option<Line<'static>>,
}

// ── Selection ──

/// A byte-range selection in the buffer, created by mouse drag.
#[derive(Debug, Clone, Copy)]
pub struct Selection {
    /// Buffer position where the selection started (fixed anchor).
    pub anchor: usize,
    /// Buffer position where the selection currently extends to (moves with drag).
    pub head: usize,
}

// ── Mouse ──

/// Result of processing a mouse event in the textarea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MouseAction {
    /// Nothing interesting happened.
    Nothing,
    /// Cursor was placed at a position (single click on plain text).
    CursorPlaced,
    /// Selection was updated (drag in progress, or double/triple click).
    SelectionUpdated,
    /// Selection was finalized — text copied to clipboard.
    /// Host should call `take_clipboard()` to retrieve it.
    SelectionFinished,
    /// Content was scrolled (mouse wheel).
    Scrolled,
}

/// Tracks consecutive clicks at the same screen position to detect
/// double-click (word select) and triple-click (line select).
#[derive(Debug)]
struct ClickTracker {
    last_time: Instant,
    last_pos: (u16, u16),
    count: u8,
}

impl Default for ClickTracker {
    fn default() -> Self {
        Self {
            last_time: Instant::now(),
            last_pos: (u16::MAX, u16::MAX),
            count: 0,
        }
    }
}

impl ClickTracker {
    /// Maximum time between clicks to count as multi-click (ms).
    const MULTI_CLICK_MS: u128 = 500;

    /// Register a click at `(col, row)`. Returns the click count (1, 2, or 3).
    fn register(&mut self, col: u16, row: u16) -> u8 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_time).as_millis();
        if elapsed < Self::MULTI_CLICK_MS && self.last_pos == (col, row) && self.count < 3 {
            self.count += 1;
        } else {
            self.count = 1;
        }
        self.last_time = now;
        self.last_pos = (col, row);
        self.count
    }
}

#[derive(Debug)]
pub struct TextArea {
    text: EditBuffer,
    wrap_cache: RefCell<Option<WrapCache>>,
    preferred_col: Option<usize>,
    elements: Vec<TextElement>,
    next_element_id: u64,
    kill_buffer: String,
    undo: UndoState,
    /// Active selection (mouse drag). `None` when no selection.
    selection: Option<Selection>,
    /// Clipboard provider — defaults to [`InternalClipboard`].
    /// Swap with [`set_clipboard_provider`](Self::set_clipboard_provider)
    /// for system clipboard support.
    clipboard_provider: Box<dyn ClipboardProvider>,
    /// Last copied text — set on copy/cut, cleared by `take_clipboard()`.
    /// This is the "notification" channel: the host calls `take_clipboard()`
    /// to detect that something was just copied.
    clipboard: Option<String>,
    /// Whether to keep the selection visible after mouse-up.
    /// When `false`, selection clears immediately on mouse-up (fully transient).
    pub keep_selection_after_mouseup: bool,
    /// Style applied to selected text.  Defaults to a tokyonight-inspired
    /// blue background (`rgb(49, 62, 115)`) with an explicit light foreground
    /// (`rgb(192, 202, 245)`) so the selection is legible regardless of the
    /// host terminal's colour scheme.
    ///
    /// Override to match your own theme, e.g.:
    /// ```ignore
    /// textarea.selection_style = Style::default().bg(Color::Rgb(60, 60, 60));
    /// ```
    pub selection_style: Style,
    /// Screen position of the last mouse-down (for distinguishing click vs drag).
    mouse_down_pos: Option<(u16, u16)>,
    /// Buffer byte position of the mouse-down anchor (for drag selection).
    drag_anchor: Option<usize>,
    /// Whether a drag is currently in progress.
    drag_active: bool,
    /// Last time drag-scroll was applied (throttle).
    last_drag_scroll: Option<Instant>,
    /// Number of drag-scroll steps taken so far (for acceleration).
    drag_scroll_steps: u32,
    /// Stored drag event for continuous drag-scroll (re-triggered on timer).
    /// Set when a drag moves outside the textarea area; cleared on mouse-up.
    pending_drag_scroll: Option<MouseEvent>,
    /// Tracks multi-click (double/triple) at the same position.
    click_tracker: ClickTracker,
    /// Internal scroll offset set by mousewheel events.  When `Some`, this
    /// overrides the external `TextAreaState.scroll` so the viewport scrolls
    /// independently of the cursor.  Cleared whenever the cursor moves
    /// (typing, navigation, click) so the viewport snaps back to follow it.
    scroll_override: Option<u16>,
    /// Whether to show a scrollbar on the right edge when content overflows.
    /// When enabled, the rightmost column is reserved for the scrollbar track
    /// and the text area wraps at `width - 1`. Defaults to `true`.
    pub show_scrollbar: bool,
    /// Style for the scrollbar track (empty space).  Defaults to a dark
    /// tokyonight-inspired background.  Override to match your theme's
    /// background when embedding the textarea in a non-default-bg context.
    pub scrollbar_track_style: Style,
    /// Style for the scrollbar thumb (draggable indicator).  Defaults to a
    /// slightly lighter tokyonight shade.  Override to match your theme.
    pub scrollbar_thumb_style: Style,
    /// Padding (in columns) between the text content and the scrollbar track.
    /// Only applies when the scrollbar is visible.  Defaults to `0`.
    pub scrollbar_padding: u16,
    /// Whether the user is currently dragging the scrollbar thumb.
    scrollbar_dragging: bool,
    /// Currently hovered element (for enter/leave detection).
    hovered_element: Option<ElementId>,
    /// Pending element event — consumed by [`poll_element_event`](Self::poll_element_event).
    pending_element_event: Option<TextElementEvent>,
    /// Columns per tab character for display width and tab→space expansion on
    /// insert. `0` leaves tabs as-is (unicode-width treats them as 0-width).
    /// Defaults to `4`, matching scrollback `appearance::tab_width`.
    tab_width: u8,
}

#[derive(Debug, Clone)]
struct WrapCache {
    width: u16,
    lines: Vec<Range<usize>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TextAreaState {
    /// Index into wrapped lines of the first visible line.
    pub scroll: u16,
}

// ── Undo/Redo ──

/// A snapshot of the textarea state for undo/redo.
#[derive(Debug, Clone)]
struct UndoEntry {
    text: String,
    cursor: usize,
    elements: Vec<TextElement>,
}

/// What kind of mutation is being performed. Used for batching consecutive
/// same-kind operations into a single undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MutationKind {
    /// Character-by-character typing, `insert_str`, `yank`.
    Insert,
    /// Backspace, delete forward.
    Delete,
    /// Ctrl+K, Ctrl+U, word-delete — always a discrete undo step.
    Kill,
    /// `insert_element`, `replace_range_with_element` — always discrete.
    Element,
    /// `set_text`, `replace_range` (host-driven) — always discrete.
    Replace,
}

/// Manages the undo/redo stacks.
#[derive(Debug)]
struct UndoState {
    stack: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    max_depth: usize,
    /// The kind of the last mutation that was checkpointed.
    last_kind: Option<MutationKind>,
    /// Cursor position *after* the last mutation completed.
    /// Used to detect cursor jumps (arrows between inserts → new undo group).
    last_cursor: usize,
    /// Whether the last inserted character was whitespace.
    /// Used to break insert batches at word boundaries (ws↔non-ws transitions).
    last_insert_ws: bool,
    /// Nesting depth for undo groups. When > 0, `pre_mutate` is suppressed.
    group_depth: usize,
    /// Snapshot taken when the outermost `begin_undo_group()` was called.
    /// Used by `end_undo_group` to push the checkpoint, or by
    /// `cancel_undo_group` to restore the pre-group state.
    group_checkpoint: Option<UndoEntry>,
}

impl Default for UndoState {
    fn default() -> Self {
        Self {
            stack: Vec::new(),
            redo: Vec::new(),
            max_depth: 100,
            last_kind: None,
            last_cursor: 0,
            last_insert_ws: false,
            group_depth: 0,
            group_checkpoint: None,
        }
    }
}

/// Whether `key` is the undo chord [`TextArea::input`] binds: lowercase
/// 'z' with Ctrl or Cmd. Uppercase 'Z' (redo) is intentionally excluded,
/// which keeps this guard disjoint from the redo arm regardless of order.
///
/// Single source for the binding: `input()`'s undo arm consumes this
/// predicate, and hosts that react to undo (e.g. retiring an undo hint)
/// call it too, so the chord and its observers cannot drift.
pub fn is_undo_input(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('z'))
        && (key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::SUPER))
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/textarea/model.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/textarea/mouse.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/textarea/navigation.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/textarea/history.rs"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/textarea/elements_wrap.rs"
));

impl WidgetRef for &TextArea {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let (cw, needs_sb) = self.content_width(area.width, area.height);
        let content_area = Rect { width: cw, ..area };
        let lines = self.wrapped_lines(cw);
        self.render_lines(content_area, buf, &lines, 0..lines.len());
        if needs_sb {
            self.render_scrollbar(area, buf, lines.len() as u16, area.height, 0);
        }
    }
}

impl StatefulWidgetRef for &TextArea {
    type State = TextAreaState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let (cw, needs_sb) = self.content_width(area.width, area.height);
        let content_area = Rect { width: cw, ..area };
        let lines = self.wrapped_lines(cw);
        let scroll = self.effective_scroll(area.height, &lines, state.scroll);
        state.scroll = scroll;

        let start = scroll as usize;
        let end = (scroll + area.height).min(lines.len() as u16) as usize;
        self.render_lines(content_area, buf, &lines, start..end);
        if needs_sb {
            self.render_scrollbar(area, buf, lines.len() as u16, area.height, scroll);
        }
    }
}

impl TextArea {
    /// Render a scrollbar in the rightmost column of `area`.
    ///
    /// Uses `tui_scrollbar::ScrollBar` rendered into a scratch ratatui-core
    /// buffer, then copies cells into the main buffer with muted styling.
    fn render_scrollbar(
        &self,
        area: Rect,
        buf: &mut Buffer,
        total_lines: u16,
        viewport_lines: u16,
        offset: u16,
    ) {
        if total_lines <= viewport_lines || area.width == 0 || area.height == 0 {
            return;
        }

        let sb_area = Rect {
            x: area.right().saturating_sub(1),
            y: area.y,
            width: 1,
            height: area.height,
        };

        let lengths = ScrollLengths {
            content_len: total_lines as usize,
            viewport_len: viewport_lines as usize,
        };
        let scrollbar = ScrollBar::vertical(lengths).offset(offset as usize);

        // Render into ratatui-core scratch buffer then copy with styling.
        let core_area = CoreRect {
            x: sb_area.x,
            y: sb_area.y,
            width: sb_area.width,
            height: sb_area.height,
        };
        let mut scratch = CoreBuffer::empty(core_area);
        (&scrollbar).render(core_area, &mut scratch);

        let track_style = self.scrollbar_track_style;
        let thumb_style = self.scrollbar_thumb_style;

        for row in 0..sb_area.height {
            let x = sb_area.x;
            let y = sb_area.y + row;
            let src = &scratch[(x, y)];
            let dst = &mut buf[(x, y)];
            let symbol = src.symbol();
            dst.set_symbol(symbol);
            if symbol == " " {
                dst.set_style(track_style);
            } else {
                dst.set_style(thumb_style);
            }
        }
    }

    fn render_lines(
        &self,
        area: Rect,
        buf: &mut Buffer,
        lines: &[Range<usize>],
        range: std::ops::Range<usize>,
    ) {
        let area_right = area.x + area.width; // exclusive right boundary
        let sel_range = self.selection_range();

        for (row, idx) in range.enumerate() {
            let r = &lines[idx];
            let y = area.y + row as u16;
            let line_range = r.start..r.end;

            // Render the line segment-by-segment (plain text → element → plain text → …)
            // using display-aware x positioning. This ensures that when an element's
            // display text is wider (or narrower) than its buffer text, all subsequent
            // content is positioned correctly.
            let mut display_x: u16 = 0; // current display column
            let mut buf_pos = line_range.start; // current position in the buffer

            // Collect elements that overlap this visual line, in order.
            let overlapping: Vec<&TextElement> = self
                .elements
                .iter()
                .filter(|e| {
                    let os = e.range.start.max(line_range.start);
                    let oe = e.range.end.min(line_range.end);
                    os < oe
                })
                .collect();

            for elem in &overlapping {
                let overlap_start = elem.range.start.max(line_range.start);
                let overlap_end = elem.range.end.min(line_range.end);

                // 1. Render plain text before this element (buf_pos..overlap_start)
                if buf_pos < overlap_start && display_x < area.width {
                    let plain = &self.text[buf_pos..overlap_start];
                    let avail = (area.width - display_x) as usize;
                    let (paint, paint_w) = paint_plain_for_display(plain, avail, self.tab_width);
                    buf.set_string(area.x + display_x, y, paint.as_ref(), Style::default());
                    display_x += paint_w as u16;
                }

                // 2. Render the element
                if display_x >= area.width {
                    buf_pos = overlap_end;
                    continue;
                }

                let avail = (area.width - display_x) as usize;

                if let Some(display) = &elem.display {
                    if overlap_start == elem.range.start {
                        // First visual line of the element — render display text.
                        let display = truncate_line_display(display, avail);
                        for span in &display.spans {
                            let content = span.content.as_ref();
                            let w = content.width() as u16;
                            if display_x >= area.width {
                                break;
                            }
                            buf.set_string(area.x + display_x, y, content, span.style);
                            display_x += w;
                        }
                    }
                    // If element spans multiple visual lines but has a display,
                    // subsequent lines show nothing for this element region (blank).
                    // display_x doesn't advance (already blank in the buffer).
                } else {
                    // No custom display: render buffer text with default element style.
                    let styled = &self.text[overlap_start..overlap_end];
                    let style = Style::default().fg(Color::Cyan);
                    let (paint, paint_w) = paint_plain_for_display(styled, avail, self.tab_width);
                    buf.set_string(area.x + display_x, y, paint.as_ref(), style);
                    display_x += paint_w as u16;
                }

                buf_pos = overlap_end;
            }

            // 3. Render any remaining plain text after the last element
            if buf_pos < line_range.end && display_x < area.width {
                let plain = &self.text[buf_pos..line_range.end];
                let avail = (area.width - display_x) as usize;
                let (paint, paint_w) = paint_plain_for_display(plain, avail, self.tab_width);
                buf.set_string(area.x + display_x, y, paint.as_ref(), Style::default());
                // Keep display_x consistent with earlier segments (selection uses
                // display_width_of_range on a second pass).
                let _painted_end = display_x.saturating_add(paint_w as u16);
                let _ = _painted_end;
            }

            // 4. Apply selection highlight (second pass over cells)
            if let Some(sel_range) = &sel_range {
                // Intersect the selection with this visual line's buffer range.
                let line_sel_start = sel_range.start.max(line_range.start);
                let line_sel_end = sel_range.end.min(line_range.end);
                if line_sel_start < line_sel_end {
                    // Compute display column range for the selected portion.
                    let col_start =
                        self.display_width_of_range(line_range.start, line_sel_start) as u16;
                    let col_end =
                        self.display_width_of_range(line_range.start, line_sel_end) as u16;
                    let col_start = col_start.min(area.width);
                    let col_end = col_end.min(area.width);
                    for cx in col_start..col_end {
                        let cell = &mut buf[(area.x + cx, y)];
                        cell.set_style(self.selection_style);
                    }
                }
            }

            let _ = area_right; // suppress unused warning (used for documentation)
        }
    }
}

/// Expand `\t` to a fixed number of spaces (`tab_width`), matching scrollback.
fn expand_tabs_with_width(text: &str, tab_width: u8) -> std::borrow::Cow<'_, str> {
    if tab_width == 0 || !text.contains('\t') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(text.replace('\t', &" ".repeat(tab_width as usize)))
}

fn grapheme_display_width_with_tab(grapheme: &str, tab_width: u8) -> usize {
    if grapheme == "\t" {
        if tab_width == 0 {
            0
        } else {
            tab_width as usize
        }
    } else {
        grapheme.width()
    }
}

fn plain_display_width_with_tab(text: &str, tab_width: u8) -> usize {
    if tab_width == 0 || !text.contains('\t') {
        return text.width();
    }
    text.graphemes(true)
        .map(|g| grapheme_display_width_with_tab(g, tab_width))
        .sum()
}

/// Clip a string to fit within `max_width` display columns (tabs = 0 width).
/// Returns a substring that is at most `max_width` columns wide.
fn clip_str_to_display_width(s: &str, max_width: usize) -> &str {
    clip_str_to_display_width_with_tab(s, max_width, 0)
}

/// Clip considering tabs as `tab_width` columns (byte index into original `s`).
fn clip_str_to_display_width_with_tab(s: &str, max_width: usize, tab_width: u8) -> &str {
    let mut width = 0;
    for (i, grapheme) in s.grapheme_indices(true) {
        let grapheme_width = grapheme_display_width_with_tab(grapheme, tab_width);
        if width + grapheme_width > max_width {
            return &s[..i];
        }
        width += grapheme_width;
    }
    s
}

/// Clip and expand tabs so paint width matches cursor/display-width math.
/// Returns (paint string, display columns used). Borrows when no expansion needed.
fn paint_plain_for_display(
    s: &str,
    max_width: usize,
    tab_width: u8,
) -> (std::borrow::Cow<'_, str>, usize) {
    let clipped = clip_str_to_display_width_with_tab(s, max_width, tab_width);
    let paint = expand_tabs_with_width(clipped, tab_width);
    let w = plain_display_width_with_tab(clipped, tab_width);
    (paint, w)
}

/// Truncate a display `Line` to fit within `max_width` columns.
///
/// If the line fits, it is returned as-is (cloned). If it overflows:
/// - Reserve 1 column for `…`.
/// - **Bracket-preservation heuristic:** if the display text ends with a closing
///   bracket (`]`, `)`, `}`, `>`), preserve it so e.g. `[Pasted ~10 lines]`
///   becomes `[Pasted ~1…]` rather than `[Pasted ~10…`.
/// - Otherwise, truncate and append `…`.
fn truncate_line_display(line: &Line<'static>, max_width: usize) -> Line<'static> {
    use ratatui::text::Span;

    let total_width: usize = line.spans.iter().map(|s| s.content.as_ref().width()).sum();
    if total_width <= max_width {
        return line.clone();
    }
    if max_width == 0 {
        return Line::default();
    }

    // Determine if we should preserve a closing bracket.
    let last_char = line
        .spans
        .iter()
        .rev()
        .find_map(|s| s.content.as_ref().chars().last());
    let (preserve_bracket, bracket_char, bracket_style) = match last_char {
        Some(ch @ (']' | ')' | '}' | '>')) => {
            // Find the style of the last span containing this char.
            let style = line.spans.last().map(|s| s.style).unwrap_or_default();
            (true, Some(ch), style)
        }
        _ => (false, None, Style::default()),
    };

    // Budget: max_width minus 1 for '…', minus 1 for bracket if preserving.
    // If max_width is too small for both ellipsis and bracket, skip bracket.
    let preserve_bracket = preserve_bracket && max_width >= 3;
    let content_budget = if preserve_bracket {
        max_width.saturating_sub(2) // 1 for …, 1 for bracket
    } else {
        max_width.saturating_sub(1) // 1 for …
    };

    let mut new_spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;

    for span in &line.spans {
        let content = span.content.as_ref();
        let sw = content.width();
        if used + sw <= content_budget {
            new_spans.push(span.clone());
            used += sw;
        } else {
            // Partially include this span without splitting a grapheme cluster.
            let remaining = content_budget - used;
            if remaining > 0 {
                let partial = clip_str_to_display_width(content, remaining);
                if !partial.is_empty() {
                    new_spans.push(Span::styled(partial.to_string(), span.style));
                }
            }
            break;
        }
    }

    // Append ellipsis (inherits style of last content span, or default).
    let ellipsis_style = new_spans.last().map(|s| s.style).unwrap_or_default();
    new_spans.push(Span::styled("…", ellipsis_style));

    // Append preserved bracket if applicable.
    if preserve_bracket && let Some(ch) = bracket_char {
        new_spans.push(Span::styled(ch.to_string(), bracket_style));
    }

    Line::from(new_spans)
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("textarea/unit");
