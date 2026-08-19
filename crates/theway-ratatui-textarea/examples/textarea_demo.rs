//! Demo for TextArea with @-file-search completion and atomic text elements.
//!
//! Run with: cargo run -p theway-ratatui-textarea --example textarea_demo
//!
//! Features demonstrated:
//! - Type `@` to trigger fuzzy file search (real files from current directory)
//! - Tab or Enter confirms selection → creates an atomic text element
//! - Up/Down to navigate results, Esc to dismiss
//! - Bracketed paste → creates paste elements
//! - Elements render as styled chips, cursor skips over them atomically
//! - Display projection: cursor column accounts for display width, not buffer width

use std::collections::HashMap;
use std::io::{self, stdout};
use std::ops::{Range, RangeInclusive};
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::cursor::{EnableBlinking, SetCursorStyle};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, StatefulWidgetRef, Widget};

use theway_ratatui_textarea::wrapping::{RtOptions, word_wrap_line};
use theway_ratatui_textarea::{
    ClipboardProvider, ElementId, ElementKind, MouseAction, TextArea, TextAreaState, TextElement,
    TextElementEventKind,
};

// ── Element kinds ──

const KIND_PASTE: ElementKind = ElementKind(1);
const KIND_FILE_REF: ElementKind = ElementKind(2);

/// Maximum number of file search results shown in the dropdown.
const MAX_RESULTS: usize = 8;

// ── System clipboard provider ──

/// Clipboard backed by `arboard` — copies/pastes to/from system clipboard.
#[derive(Debug)]
struct ArboardClipboard;

impl ClipboardProvider for ArboardClipboard {
    fn get(&mut self) -> Option<String> {
        arboard::Clipboard::new().ok()?.get_text().ok()
    }

    fn set(&mut self, text: &str) {
        if let Ok(mut clip) = arboard::Clipboard::new() {
            let _ = clip.set_text(text);
        }
    }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// File search
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// A single fuzzy-matched file result.
struct SearchResult {
    path: String,
    score: i64,
    /// Character indices in `path` that matched the query (for highlighting).
    indices: Vec<usize>,
}

/// Manages the file list, fuzzy matcher, and dropdown state for @-completion.
struct FileSearch {
    all_files: Vec<String>,
    matcher: SkimMatcherV2,
    results: Vec<SearchResult>,
    selected: usize,
}

/// Context extracted from textarea buffer describing an active @-completion trigger.
struct FileSearchContext {
    /// Byte range in the buffer covering `@query` (includes the `@`).
    range: Range<usize>,
    /// The query text (characters after `@`, up to cursor position).
    query: String,
}

impl FileSearch {
    fn new() -> Self {
        let all_files = collect_files();
        Self {
            all_files,
            matcher: SkimMatcherV2::default(),
            results: Vec::new(),
            selected: 0,
        }
    }

    /// Re-run fuzzy matching against `query` and update the results list.
    fn update(&mut self, query: &str) {
        self.results.clear();
        if query.is_empty() {
            // Show first N files alphabetically when query is empty.
            for path in self.all_files.iter().take(MAX_RESULTS) {
                self.results.push(SearchResult {
                    path: path.clone(),
                    score: 0,
                    indices: Vec::new(),
                });
            }
        } else {
            let mut scored: Vec<_> = self
                .all_files
                .iter()
                .filter_map(|path| {
                    self.matcher
                        .fuzzy_indices(path, query)
                        .map(|(score, indices)| SearchResult {
                            path: path.clone(),
                            score,
                            indices,
                        })
                })
                .collect();
            scored.sort_by_key(|b| std::cmp::Reverse(b.score));
            scored.truncate(MAX_RESULTS);
            self.results = scored;
        }
        self.clamp_selection();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let max = self.results.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    fn selected_path(&self) -> Option<&str> {
        self.results.get(self.selected).map(|r| r.path.as_str())
    }

    fn is_visible(&self) -> bool {
        !self.results.is_empty()
    }

    fn clear(&mut self) {
        self.results.clear();
        self.selected = 0;
    }

    fn clamp_selection(&mut self) {
        if self.results.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.results.len() - 1);
        }
    }

    /// Height needed for the dropdown (0 when hidden).
    fn dropdown_height(&self) -> u16 {
        if self.results.is_empty() {
            0
        } else {
            // results + 2 for the border
            (self.results.len() as u16 + 2).min(MAX_RESULTS as u16 + 2)
        }
    }
}

/// Walk the current directory using the `ignore` crate (respects .gitignore).
fn collect_files() -> Vec<String> {
    let mut files = Vec::new();
    for entry in ignore::Walk::new(".") {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_none_or(|ft| !ft.is_file()) {
            continue;
        }
        let path = entry.path().display().to_string();
        let path = path.strip_prefix("./").unwrap_or(&path).to_string();
        files.push(path);
    }
    files.sort();
    files
}

/// Compute the @-completion context from current textarea state.
///
/// Scans backward from `cursor` looking for an `@` that could be a file search
/// trigger. Returns `None` if no valid context is found.
fn compute_file_search_context(
    text: &str,
    cursor: usize,
    elements: &[TextElement],
) -> Option<FileSearchContext> {
    if cursor == 0 {
        return None;
    }

    let at_idx = text[..cursor].rfind('@')?;

    // Don't trigger if the @ is inside an existing element (already confirmed).
    if elements
        .iter()
        .any(|e| at_idx >= e.range.start && at_idx < e.range.end)
    {
        return None;
    }

    // Don't trigger if preceded by alphanumeric or _ (e.g. email-like `user@`).
    if let Some(ch) = text[..at_idx].chars().next_back()
        && (ch.is_alphanumeric() || ch == '_')
    {
        return None;
    }

    // Find the end of the @-token (whitespace or punctuation terminates).
    let token_end = text[at_idx + 1..]
        .char_indices()
        .find_map(|(offset, ch)| {
            (ch.is_whitespace() || matches!(ch, ',' | ';')).then_some(at_idx + 1 + offset)
        })
        .unwrap_or(text.len());

    // Cursor must be within the token.
    if cursor > token_end {
        return None;
    }

    let query = text[at_idx + 1..cursor].to_owned();
    Some(FileSearchContext {
        range: at_idx..token_end,
        query,
    })
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Line select mode (file preview + line range picking)
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[derive(Clone, Copy, PartialEq)]
enum SelectionState {
    /// No selection active.
    None,
    /// First `v`: anchor line (0-indexed). Range extends as cursor moves.
    Selecting(usize),
    /// Second `v`: range locked (0-indexed, inclusive, sorted).
    Locked(usize, usize),
}

/// Modal state for the file preview / line-range picker.
struct LineSelectMode {
    file_path: String,
    lines: Vec<String>,
    cursor_line: usize, // 0-indexed
    scroll_top: usize,  // 0-indexed first visible line
    viewport_height: usize,
    goto_buf: String,
    selection: SelectionState,
    element_id: ElementId,
}

impl LineSelectMode {
    /// Load a file and create a new line-select session.
    fn open(file_path: String, element_id: ElementId) -> Option<Self> {
        let content = std::fs::read_to_string(&file_path).ok()?;
        let lines: Vec<String> = content.lines().map(String::from).collect();
        if lines.is_empty() {
            return None;
        }
        Some(Self {
            file_path,
            lines,
            cursor_line: 0,
            scroll_top: 0,
            viewport_height: 20,
            goto_buf: String::new(),
            selection: SelectionState::None,
            element_id,
        })
    }

    fn total_lines(&self) -> usize {
        self.lines.len()
    }

    fn move_cursor(&mut self, delta: isize) {
        let max = self.total_lines().saturating_sub(1) as isize;
        self.cursor_line = (self.cursor_line as isize + delta).clamp(0, max) as usize;
        self.ensure_visible();
    }

    fn goto_line(&mut self, line_1indexed: usize) {
        self.cursor_line = line_1indexed
            .saturating_sub(1)
            .min(self.total_lines().saturating_sub(1));
        self.center_cursor();
    }

    fn ensure_visible(&mut self) {
        if self.cursor_line < self.scroll_top {
            self.scroll_top = self.cursor_line;
        } else if self.cursor_line >= self.scroll_top + self.viewport_height {
            self.scroll_top = self.cursor_line + 1 - self.viewport_height;
        }
    }

    fn center_cursor(&mut self) {
        let half = self.viewport_height / 2;
        self.scroll_top = self.cursor_line.saturating_sub(half);
        let max_scroll = self.total_lines().saturating_sub(self.viewport_height);
        self.scroll_top = self.scroll_top.min(max_scroll);
    }

    fn toggle_selection(&mut self) -> SelectionState {
        let prev = self.selection;
        self.selection = match self.selection {
            SelectionState::None => SelectionState::Selecting(self.cursor_line),
            SelectionState::Selecting(anchor) => {
                let (s, e) = sorted(anchor, self.cursor_line);
                SelectionState::Locked(s, e)
            }
            SelectionState::Locked(_, _) => SelectionState::Selecting(self.cursor_line),
        };
        prev
    }

    /// Get the current effective line range (1-indexed, inclusive).
    fn effective_range(&self) -> Option<RangeInclusive<usize>> {
        match self.selection {
            SelectionState::None => None,
            SelectionState::Selecting(anchor) => {
                let (s, e) = sorted(anchor, self.cursor_line);
                Some((s + 1)..=(e + 1))
            }
            SelectionState::Locked(s, e) => Some((s + 1)..=(e + 1)),
        }
    }

    /// Check if a 0-indexed line is in the current selection.
    fn is_selected(&self, line: usize) -> bool {
        match self.selection {
            SelectionState::None => false,
            SelectionState::Selecting(anchor) => {
                let (s, e) = sorted(anchor, self.cursor_line);
                line >= s && line <= e
            }
            SelectionState::Locked(s, e) => line >= s && line <= e,
        }
    }

    /// If the selection covers every line, clear it (whole file = no range).
    fn check_select_all(&mut self) {
        let total = self.total_lines();
        let covers_all = match self.selection {
            SelectionState::Selecting(anchor) => {
                let (s, e) = sorted(anchor, self.cursor_line);
                s == 0 && e + 1 >= total
            }
            SelectionState::Locked(s, e) => s == 0 && e + 1 >= total,
            SelectionState::None => false,
        };
        if covers_all {
            self.selection = SelectionState::None;
        }
    }
}

fn sorted(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

// ── File-ref helpers ──

/// Parse element text like `@foo.rs:123-456` into (path, optional range).
fn parse_file_ref(element_text: &str) -> (&str, Option<RangeInclusive<usize>>) {
    let text = element_text.strip_prefix('@').unwrap_or(element_text);
    if let Some(colon) = text.rfind(':') {
        let suffix = &text[colon + 1..];
        if let Some(range) = parse_line_range_str(suffix) {
            return (&text[..colon], Some(range));
        }
    }
    (text, None)
}

fn parse_line_range_str(s: &str) -> Option<RangeInclusive<usize>> {
    if let Some(dash) = s.find('-') {
        let start: usize = s[..dash].parse().ok()?;
        let end: usize = s[dash + 1..].parse().ok()?;
        Some(start..=end)
    } else {
        let line: usize = s.parse().ok()?;
        Some(line..=line)
    }
}

fn build_file_ref_text(path: &str, range: Option<&RangeInclusive<usize>>) -> String {
    match range {
        None => format!("@{path}"),
        Some(r) if r.start() == r.end() => format!("@{path}:{}", r.start()),
        Some(r) => format!("@{path}:{}-{}", r.start(), r.end()),
    }
}

fn build_file_ref_display(path: &str, range: Option<&RangeInclusive<usize>>) -> Line<'static> {
    let bg = Color::Rgb(30, 50, 30);
    let mut spans = vec![
        Span::styled("@", Style::default().fg(Color::Green).bg(bg).bold()),
        Span::styled(path.to_string(), Style::default().fg(Color::Green).bg(bg)),
    ];
    if let Some(r) = range {
        let range_text = if r.start() == r.end() {
            format!(":{}", r.start())
        } else {
            format!(":{}‑{}", r.start(), r.end())
        };
        spans.push(Span::styled(
            range_text,
            Style::default().fg(Color::Rgb(230, 120, 100)).bg(bg),
        ));
    }
    Line::from(spans)
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// App
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// Result of processing an input event.
enum EventResult {
    /// Continue running, redraw the UI.
    Redraw,
    /// Continue running, nothing changed — skip redraw (lets cursor blink).
    Unchanged,
    /// Exit the application.
    Quit,
}

/// Host-side metadata for an element.
struct ElementMeta {
    description: String,
}

struct DemoApp {
    textarea: TextArea,
    textarea_state: TextAreaState,
    element_meta: HashMap<ElementId, ElementMeta>,
    status: String,
    file_search: FileSearch,
    /// Whether the file-search dropdown is logically active.
    fs_active: bool,
    /// Modal line-select / file-preview mode.
    line_select: Option<LineSelectMode>,
    /// Last render area for the textarea (needed for mouse→buffer mapping).
    textarea_area: Rect,
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/textarea_demo/event_search.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/textarea_demo/key_input.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/textarea_demo/line_select.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/examples/textarea_demo/rendering.rs"
));
/// Split `s` at newlines, push text segments with `text_style` and
/// literal `\n` markers with `nl_style` (dim).
fn push_with_visible_newlines<'a>(
    s: &'a str,
    text_style: Style,
    nl_style: Style,
    out: &mut Vec<Span<'a>>,
) {
    let mut first = true;
    for part in s.split('\n') {
        if !first {
            out.push(Span::styled("\\n", nl_style));
        }
        if !part.is_empty() {
            out.push(Span::styled(part, text_style));
        }
        first = false;
    }
}

fn main() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    stdout().execute(EnableBracketedPaste)?;
    stdout().execute(EnableMouseCapture)?;
    stdout().execute(EnableBlinking)?;
    stdout().execute(SetCursorStyle::BlinkingBlock)?;

    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal);

    let _ = stdout().execute(DisableMouseCapture);
    let _ = stdout().execute(DisableBracketedPaste);
    let _ = stdout().execute(SetCursorStyle::DefaultUserShape);
    let _ = terminal::disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);

    if let Err(ref e) = result {
        eprintln!("textarea_demo exited with error: {e}");
    }

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut app = DemoApp::new();
    app.render(terminal)?;

    loop {
        // The textarea tells us when it needs a timer tick (e.g. for
        // continuous drag-scrolling).  Use its timeout for poll, falling
        // back to a generous default that lets the cursor blink.
        let timeout = app
            .textarea
            .poll_timeout_ms()
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(100));

        if crossterm::event::poll(timeout)? {
            let event = crossterm::event::read()?;
            match app.handle_event(event) {
                EventResult::Quit => break,
                EventResult::Redraw => app.render(terminal)?,
                EventResult::Unchanged => {}
            }
        }

        // Whether we processed an event or timed out, check if the
        // textarea has pending timer work (e.g. continuous drag-scroll).
        // The textarea's internal throttle prevents this from firing
        // too fast — it'll return Nothing if not enough time has passed.
        if app.textarea.poll_timeout_ms().is_some() {
            let action = app.textarea.tick(app.textarea_area, app.textarea_state);
            if matches!(action, MouseAction::SelectionUpdated) {
                if let Some(text) = app.textarea.selected_text() {
                    let chars = text.chars().count();
                    app.status = format!("Selecting… ({chars} chars)");
                }
                app.render(terminal)?;
            }
        }
    }

    Ok(())
}
