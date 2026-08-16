//! Ratatui rendering for the conversation feed — the terminal rendering lives
//! here in the TUI; the UI-agnostic model lives in `theway_transport::feed`.
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use theway_transport::feed::{Block, Level, display_prefix, wrap_str};

use crate::ui::selection::{self, FeedSelection};
use crate::ui::theme::{BlockAlign, BlockTheme, Theme};

/// Grok tokyonight palette values (xai-grok-pager-render theme/tokyonight.rs).
const ACCENT_USER: Color = Color::Rgb(122, 162, 247); // BLUE — user `❯` prefix
const ACCENT_ASSISTANT: Color = Color::Rgb(187, 154, 247); // MAGENTA — `ai ▸` prefix
const ACCENT_TOOL: Color = Color::Rgb(115, 122, 162); // DARK5 — tool name
const TEXT_PRIMARY: Color = Color::Rgb(192, 202, 245); // FG — body text
const BG_HIGHLIGHT: Color = Color::Rgb(41, 46, 66); // BG_HIGHLIGHT — user band / selection

// ── Theme-role defaults (issue #43) ─────────────────────────────────────────
// The pre-theme hardcoded colors, kept as the single source of truth for
// `Theme::default()`: a build without `~/.theway/theme.toml` renders exactly
// as before.
pub(crate) const USER_TEXT_DEFAULT: Color = TEXT_PRIMARY;
pub(crate) const USER_BG_DEFAULT: Color = BG_HIGHLIGHT;
pub(crate) const ASSISTANT_TEXT_DEFAULT: Option<Color> = None;
pub(crate) const ASSISTANT_PREFIX_DEFAULT: Color = ACCENT_ASSISTANT;
pub(crate) const TOOL_TITLE_DEFAULT: Color = ACCENT_TOOL;
pub(crate) const TOOL_ARGS_DEFAULT: Color = Color::DarkGray;
pub(crate) const TOOL_RESULT_DEFAULT: Color = Color::Green;
pub(crate) const TOOL_ERROR_DEFAULT: Color = Color::Red;
pub(crate) const TOOL_RUNNING_BG_DEFAULT: Option<Color> = None;
pub(crate) const TOOL_SUCCESS_BG_DEFAULT: Option<Color> = None;
pub(crate) const TOOL_ERROR_BG_DEFAULT: Option<Color> = None;
pub(crate) const THINKING_TEXT_DEFAULT: Color = Color::DarkGray;
pub(crate) const THINKING_BG_DEFAULT: Option<Color> = None;

pub(crate) const USER_PREFIX: &str = "\u{276F} "; // ❯ (2 cols, grok prompt_arrow)
pub(crate) const AI_PREFIX: &str = "ai \u{25b8} "; // ai ▸
pub(crate) const TOOL_PREFIX: &str = "\u{23f5} "; // ⏵
const USER_BAND_INDENT: &str = "  ";

const USER_STYLE: Style = Style::new().fg(ACCENT_USER).add_modifier(Modifier::BOLD);
/// Default thinking style; the streaming thinking path in `feed_cache`
/// reuses this const (theme-aware colors apply to one-shot renders).
pub(crate) const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
/// Default assistant prefix style; the streaming markdown path in
/// `feed_cache` reuses this const (theme-aware colors apply to one-shot
/// renders).
pub(crate) const AI_PREFIX_STYLE: Style = Style::new()
    .fg(ACCENT_ASSISTANT)
    .add_modifier(Modifier::BOLD);
pub(crate) const RESULT_SUMMARY_STYLE: Style = Style::new().fg(Color::DarkGray);

fn user_body_style(theme: &Theme) -> Style {
    Style::new().fg(theme.user_text)
}
fn band_style(theme: &Theme) -> Style {
    Style::new().bg(theme.user_bg)
}
fn ai_prefix_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.assistant_prefix)
        .add_modifier(Modifier::BOLD)
}
fn tool_name_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.tool_title)
        .add_modifier(Modifier::BOLD)
}
fn tool_args_style(theme: &Theme) -> Style {
    Style::new().fg(theme.tool_args)
}
fn thinking_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.thinking_text)
        .add_modifier(Modifier::ITALIC)
}

/// How `Block::Thinking` renders in the feed (Ctrl+O cycles).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ThinkingMode {
    /// Full thinking text (default).
    #[default]
    Full,
    /// Peek window: header + the last few lines only.
    Peek,
    /// Skipped entirely.
    Hidden,
}

/// Renderer switches owned by the TUI app state.
///
/// `PartialEq` is hand-implemented over the structural switches only
/// (`thinking_mode` / `tools_expanded` / `theme`): the per-frame counters
/// (`thinking_cps` / `thinking_input_tokens` / `thinking_output_tokens` /
/// `spinner_phase`) change every frame while a turn streams and must NOT
/// participate in equality — otherwise the feed cache sees a new option set
/// every frame and the #34/#35 incremental rendering degrades to full
/// re-renders. Streaming tails re-render their stats line with fresh
/// counters each frame; frozen historical blocks keep the values they were
/// rendered with. The theme is structural: it changes only at startup (or
/// on reload), so any theme change invalidates the whole cache.
#[derive(Clone, Copy, Debug, Default)]
pub struct FeedRenderOptions {
    pub thinking_mode: ThinkingMode,
    /// Tool results: collapsed to a bordered preview unless expanded (Ctrl+T).
    pub tools_expanded: bool,
    /// Thinking-block throughput (chars/sec over the last 1s window) shown on
    /// the stats line; sourced by the CpsMeter (node 3-spinner).
    pub thinking_cps: f64,
    /// Last-turn input token count shown on the thinking stats line.
    pub thinking_input_tokens: u64,
    /// Last-turn output token count shown on the thinking stats line.
    pub thinking_output_tokens: u64,
    /// Rainbow spinner animation phase (node 3-spinner); passthrough, not
    /// consumed by block rendering. Dead until a consumer wires it — kept in
    /// the option set so per-frame animation state travels with the render
    /// switches (and excluded from `PartialEq` like the other per-frame
    /// counters).
    #[allow(dead_code)]
    pub spinner_phase: u32,
    /// Theme color roles + block layout + composer style (issues #43 + #49),
    /// loaded once at startup into `App.theme` and threaded into every
    /// render so the feed cache fingerprints theme changes.
    pub theme: Theme,
}

/// Structural equality only (issue #44 + #49): per-frame counters
/// (cps / in / out / spinner_phase) are excluded so the feed cache keeps its
/// incremental rendering across frames; the theme participates because it
/// changes colors and layout.
impl PartialEq for FeedRenderOptions {
    fn eq(&self, other: &Self) -> bool {
        self.thinking_mode == other.thinking_mode
            && self.tools_expanded == other.tools_expanded
            && self.theme == other.theme
    }
}

/// Lines shown in the thinking peek window.
pub(crate) const THINKING_PEEK_LINES: usize = 3;

/// Left border + indent prefixed to each tool result preview line.
const TOOL_RESULT_BORDER: &str = "   \u{2502} ";
/// Tool result preview height before the `…(N more lines)` elision row.
const TOOL_RESULT_PREVIEW_LINES: usize = 5;

pub fn style_for_level(level: Level) -> Style {
    match level {
        Level::Output => Style::default(),
        Level::System => Style::default().fg(Color::DarkGray),
        Level::Error => Style::default().fg(Color::Red),
        Level::Note => Style::default().fg(Color::Green),
        Level::Header => Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
        Level::Qr => Style::default(),
    }
}

/// Split `text` on newlines, word-wrap each paragraph to `width`, and push styled lines. An
/// optional `prefix` is prepended to the very first paragraph (e.g. `you ▸ `).
pub fn push_paragraphs(
    out: &mut Vec<Line<'static>>,
    text: &str,
    style: Style,
    prefix: Option<&str>,
    width: usize,
) {
    for (i, para) in text.split('\n').enumerate() {
        let owned;
        let para = if i == 0 {
            if let Some(p) = prefix {
                owned = format!("{p}{para}");
                owned.as_str()
            } else {
                para
            }
        } else {
            para
        };
        for row in wrap_str(para, width) {
            out.push(Line::styled(row, style));
        }
    }
}

/// Markdown style for assistant feed blocks.
///
/// `theway-markdown` derives an all-plain `MarkdownStyle::default()` (it is a
/// customization point for full-palette consumers such as the pager theme);
/// the feed needs effect-based styles so bold/italic/headings/inline code
/// actually render and pretty mode hides the syntax markers. Effects-only
/// (no foreground colors) keeps the feed legible on every terminal; colors
/// are then adapted to the terminal's color capabilities via [`adapt`].
///
/// [`adapt`]: theway_markdown::MarkdownStyle::adapt
pub(crate) fn markdown_style() -> theway_markdown::MarkdownStyle {
    use anstyle::Style as AStyle;
    theway_markdown::MarkdownStyle {
        heading_inner: [AStyle::new().bold(); 6],
        heading_outer: [AStyle::new().hidden(); 6],
        strong_inner: AStyle::new().bold(),
        strong_outer: AStyle::new().hidden(),
        emphasis_inner: AStyle::new().italic(),
        emphasis_outer: AStyle::new().hidden(),
        strikethrough_inner: AStyle::new().strikethrough(),
        strikethrough_outer: AStyle::new().hidden(),
        inline_code_inner: AStyle::new().bold(),
        inline_code_outer: AStyle::new().hidden(),
        blockquote_outer: AStyle::new().dimmed(),
        task_checked: AStyle::new(),
        task_unchecked: AStyle::new().dimmed(),
        list_item: AStyle::new().dimmed(),
        rule: AStyle::new(),
        link_outer: AStyle::new(),
        link_text: AStyle::new().bold(),
        link_url: AStyle::new().dimmed(),
        link_title: AStyle::new(),
        code_outer: AStyle::new().hidden(),
        code_language: AStyle::new().hidden(),
        code_untagged: AStyle::new(),
        code_background: AStyle::new(),
        table_outer: AStyle::new().bold(),
        text: AStyle::new(),
        math: AStyle::new().italic(),
    }
    .adapt()
}

/// Table border glyphs: pretty-mode tables render with box-drawing borders
/// (`TableBorders` BOX default, DOUBLE included for safety). Lines containing
/// these glyphs are table rows and must stay verbatim (no re-wrapping).
const TABLE_BORDER_CHARS: &[char] = &[
    '─', '│', '┌', '┐', '└', '┘', '┬', '┴', '├', '┤', '┼', '═', '║', '╔', '╗', '╚', '╝', '╦', '╩',
    '╠', '╣', '╬',
];

fn is_table_line(line: &Line<'_>) -> bool {
    line.spans.iter().any(|span| {
        span.content
            .chars()
            .any(|c| TABLE_BORDER_CHARS.contains(&c))
    })
}

/// One row produced by [`wrap_str_ranges`]: the source byte range it covers
/// (rows are trimmed at break points, so ranges exclude the dropped boundary
/// whitespace).
struct WrappedRow {
    range: std::ops::Range<usize>,
}

/// [`wrap_str`] equivalent that also reports each row's byte range in `text`.
/// Kept byte-for-byte in sync with `wrap_str` so hyperlink column ranges and
/// span styles can be projected onto the wrapped rows.
fn wrap_str_ranges(text: &str, width: usize) -> Vec<WrappedRow> {
    let width = width.max(1);
    if text.is_empty() {
        return vec![WrappedRow { range: 0..0 }];
    }
    let mut rows: Vec<WrappedRow> = Vec::new();
    let mut cur = String::new();
    let mut chunk_start = 0usize;
    let mut chunk_end = 0usize;
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;
    for ch in text.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            if let Some(bp) = last_space.take() {
                let rest_orig = cur.split_off(bp);
                let rest = rest_orig.trim_start_matches(' ').to_string();
                let trimmed_front = rest_orig.len() - rest.len();
                let done_len = cur.trim_end().len();
                rows.push(WrappedRow {
                    range: chunk_start..chunk_start + done_len,
                });
                cur = rest;
                chunk_start += bp + trimmed_front;
                cur_w = unicode_width::UnicodeWidthStr::width(cur.as_str());
            } else {
                rows.push(WrappedRow {
                    range: chunk_start..chunk_end,
                });
                cur.clear();
                chunk_start = chunk_end;
                cur_w = 0;
            }
        }
        cur.push(ch);
        chunk_end += ch.len_utf8();
        cur_w += cw;
        if ch == ' ' {
            last_space = Some(cur.len());
        }
    }
    rows.push(WrappedRow {
        range: chunk_start..chunk_end,
    });
    rows
}

/// Where a pre-wrap rendered line landed in the output rows.
struct MappedLine {
    /// First output row index for this pre-wrap line.
    start: usize,
    /// Pre-wrap source text (prefix included for the first line) when the
    /// line was re-wrapped; `None` when the line was pushed verbatim.
    source: Option<String>,
    /// Wrapped rows with their byte ranges in `source` (empty when verbatim).
    rows: Vec<WrappedRow>,
}

/// Render assistant markdown through `theway-markdown` (one-shot full render,
/// pretty mode) and push width-wrapped `ratatui` lines.
///
/// `prefix` (e.g. `ai ▸ `) is prepended to the first rendered line only, in
/// `prefix_style`. Fenced-code body lines (per the renderer's `code_blocks`
/// output ranges) and table rows stay verbatim; every other line wraps with
/// `wrap_str`. The width-aware render entry passes `max_table_width` so fenced
/// `mermaid` diagrams come out sized to the feed (over-wide graphs fall back
/// to a framed source box). Link underlines come from the renderer's
/// `hyperlinks` output — the assistant block does not go through the regex
/// URL scan.
pub fn push_markdown(
    out: &mut Vec<Line<'static>>,
    text: &str,
    prefix: &str,
    prefix_style: Style,
    width: usize,
) {
    let (rendered, _checkpoint) = theway_markdown::render_markdown_ratatui_full_width(
        text,
        markdown_style(),
        true,
        Some(theway_markdown::default_syntect()),
        Some(width),
    );
    let width = width.max(1);
    for (i, line) in rendered.lines.into_iter().enumerate() {
        push_rendered_markdown_line(
            out,
            i,
            line,
            prefix,
            prefix_style,
            width,
            &rendered.code_blocks,
            &rendered.hyperlinks,
        );
    }
}

/// Process ONE rendered markdown line into width-wrapped, prefixed, underlined
/// `ratatui` lines. Shared by the one-shot [`push_markdown`] path and the
/// streaming tail renderer (`feed_cache`): frozen lines are processed exactly
/// once, unfrozen tail lines once per frame.
pub(crate) fn push_rendered_markdown_line(
    out: &mut Vec<Line<'static>>,
    line_index: usize,
    line: Line<'static>,
    prefix: &str,
    prefix_style: Style,
    width: usize,
    code_blocks: &[theway_markdown::CodeBlockSpan],
    hyperlinks: &[theway_markdown::HyperlinkTarget],
) {
    let width = width.max(1);
    let first = line_index == 0;
    let in_code = code_blocks
        .iter()
        .any(|cb| cb.output_line_range.contains(&line_index));
    let keep = in_code || is_table_line(&line);

    let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let source = if first {
        format!("{prefix}{line_text}")
    } else {
        line_text
    };

    let mapped = if keep || unicode_width::UnicodeWidthStr::width(source.as_str()) <= width {
        let mut line = line;
        if first {
            line.spans
                .insert(0, Span::styled(prefix.to_string(), prefix_style));
        }
        let start = out.len();
        out.push(line);
        MappedLine {
            start,
            source: None,
            rows: Vec::new(),
        }
    } else {
        // Re-wrap the line with `wrap_str` semantics but re-apply each
        // span's style through the row byte ranges, so inline styles
        // (bold/italic/code/link) survive the wrap instead of degrading
        // to a single base style.
        let prefix_len = if first { prefix.len() } else { 0 };
        let mut pieces: Vec<(std::ops::Range<usize>, Style)> =
            Vec::with_capacity(line.spans.len() + 1);
        if first {
            pieces.push((0..prefix_len, prefix_style));
        }
        let mut offset = prefix_len;
        for span in &line.spans {
            let end = offset + span.content.len();
            pieces.push((offset..end, span.style));
            offset = end;
        }
        let rows = wrap_str_ranges(&source, width);
        let start = out.len();
        for row in &rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (range, style) in &pieces {
                let overlap = range.start.max(row.range.start)..range.end.min(row.range.end);
                if overlap.start < overlap.end {
                    spans.push(Span::styled(source[overlap].to_string(), *style));
                }
            }
            out.push(Line::from(spans));
        }
        MappedLine {
            start,
            source: Some(source),
            rows,
        }
    };

    // Underline hyperlinks on this line: the renderer reports each link as
    // (pre-wrap line index, display-column range), which maps directly onto
    // verbatim rows and through the byte ranges onto wrapped rows.
    let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);
    for link in hyperlinks
        .iter()
        .filter(|link| link.line_index == line_index)
    {
        let shift = if first { prefix_width } else { 0 };
        let start_col = link.column_range.start + shift;
        let end_col = link.column_range.end + shift;
        match &mapped.source {
            Some(source) => {
                let byte_start =
                    theway_pager_render::line_utils::byte_offset_at_width(source, start_col);
                let byte_end =
                    theway_pager_render::line_utils::byte_offset_at_width(source, end_col);
                for (j, row) in mapped.rows.iter().enumerate() {
                    let overlap = byte_start.max(row.range.start)..byte_end.min(row.range.end);
                    if overlap.start >= overlap.end {
                        continue;
                    }
                    let row_idx = mapped.start + j;
                    let row_start_width =
                        unicode_width::UnicodeWidthStr::width(&source[..row.range.start]);
                    let c0 = unicode_width::UnicodeWidthStr::width(&source[..overlap.start])
                        - row_start_width;
                    let c1 = unicode_width::UnicodeWidthStr::width(&source[..overlap.end])
                        - row_start_width;
                    underline_range(&mut out[row_idx], c0, c1);
                }
            }
            None => underline_range(&mut out[mapped.start], start_col, end_col),
        }
    }
}

/// Render the whole feed to width-wrapped `ratatui` lines, ready to scroll/draw.
///
/// The production render path uses the block cache (`feed_cache`) instead;
/// this whole-feed renderer stays for tests (reference composition with
/// separators) and diagnostics.
///
/// Grok-style block styling (issue #33): user rows carry a `❯` accent prefix on
/// an elevated band, tool calls are `⏵ name` + dim args single-liners, tool
/// results collapse to a one-line summary unless `tools_expanded`, and
/// thinking blocks honor `thinking_mode`. Timestamps are dropped for
/// conversational blocks (grok shows none); plain status lines keep theirs.
#[cfg(test)]
pub fn lines(
    feed: &theway_transport::feed::Feed,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut previous: Option<&Block> = None;
    for block in feed.blocks() {
        if theway_transport::feed::should_separate(previous, block, !out.is_empty()) {
            out.push(Line::raw(""));
        }
        out.extend(render_block(block, width, opts));
        previous = Some(block);
    }
    out
}

/// Render ONE feed block to width-wrapped lines (no inter-block separator).
/// The feed render cache calls this per dirty block; [`lines`] composes it
/// with separators. Assistant blocks skip the URL regex scan (their underlines
/// come from the markdown renderer's hyperlinks); every other block scans its
/// own lines once, so the per-block result is stable and cacheable.
///
/// Tool / ToolResult / Thinking blocks carry the theme block layout (issue
/// #49): background, padding columns and left/right alignment, with the
/// background spanning every block row at full width. Colors come from the
/// theme roles in `opts.theme` (issue #43).
pub(crate) fn render_block(
    block: &Block,
    width: usize,
    opts: &FeedRenderOptions,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let theme = &opts.theme;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut assistant_rows: Vec<std::ops::Range<usize>> = Vec::new();
    match block {
        Block::User { text, .. } => push_user_block(&mut out, text, width, theme),
        Block::Assistant { text, .. } => {
            // Assistant blocks are markdown: one-shot pretty render via
            // theway-markdown, link underlines from the renderer's
            // hyperlinks, verbatim code/table/mermaid rows, wrapped prose.
            let start = out.len();
            push_markdown(&mut out, text, AI_PREFIX, ai_prefix_style(theme), width);
            // `assistant_text` role: fallback foreground for spans the
            // markdown renderer left uncolored (syntax colors win).
            if let Some(fg) = theme.assistant_text {
                for line in &mut out[start..] {
                    for span in &mut line.spans {
                        if span.style.fg.is_none() {
                            span.style = span.style.fg(fg);
                        }
                    }
                }
            }
            assistant_rows.push(start..out.len());
        }
        Block::Thinking { text, .. } => {
            let bg = theme.thinking.bg.or(theme.thinking_bg);
            let content_w = block_content_width(width, bg, theme.thinking.padding);
            let mut rows: Vec<Line<'static>> = Vec::new();
            match opts.thinking_mode {
                ThinkingMode::Hidden => {}
                ThinkingMode::Peek => push_thinking_peek(&mut rows, text, opts, content_w),
                ThinkingMode::Full => {
                    push_thinking_stats_line(&mut rows, text, opts, content_w);
                    push_paragraphs(
                        &mut rows,
                        text,
                        thinking_style(theme),
                        Some(TOOL_PREFIX),
                        content_w,
                    )
                }
            }
            apply_block_layout(&mut rows, width, bg, &theme.thinking);
            out.extend(rows);
        }
        Block::Tool { name, args, .. } => {
            let bg = theme.tool.bg.or(theme.tool_running_bg);
            let content_w = block_content_width(width, bg, theme.tool.padding);
            let mut spans = vec![Span::styled(
                format!("{TOOL_PREFIX}{name}"),
                tool_name_style(theme),
            )];
            if !args.is_empty() {
                spans.push(Span::styled(format!(" {args}"), tool_args_style(theme)));
            }
            let mut line = Line::from(spans);
            truncate_line(&mut line, content_w);
            let mut rows = vec![line];
            apply_block_layout(&mut rows, width, bg, &theme.tool);
            out.extend(rows);
        }
        Block::ToolResult {
            lines, is_error, ..
        } => {
            let bg = theme.tool.bg.or(if *is_error {
                theme.tool_error_bg
            } else {
                theme.tool_success_bg
            });
            let content_w = block_content_width(width, bg, theme.tool.padding);
            let style = if *is_error {
                Style::default().fg(theme.tool_error)
            } else {
                Style::default().fg(theme.tool_result)
            };
            let mut rows: Vec<Line<'static>> = Vec::new();
            if opts.tools_expanded {
                push_tool_result_expanded(&mut rows, lines, content_w, style);
            } else {
                push_tool_result_preview(&mut rows, lines, *is_error, content_w, theme);
            }
            apply_block_layout(&mut rows, width, bg, &theme.tool);
            out.extend(rows);
        }
        Block::Plain {
            text,
            level,
            timestamp,
        } => {
            let prefix = timestamp.as_deref().map(|ts| display_prefix(Some(ts), ""));
            push_paragraphs(
                &mut out,
                text,
                style_for_level(*level),
                prefix.as_deref(),
                width,
            );
        }
    }
    underline_links(&mut out, &assistant_rows);
    out
}

/// Content width for a tool/thinking block row: with a background active the
/// padding columns are reserved on both sides; without one the block keeps
/// the classic full-width layout (theme padding is part of the background
/// fill and renders nothing on its own).
fn block_content_width(width: usize, bg: Option<Color>, padding: u16) -> usize {
    if bg.is_some() {
        width
            .saturating_sub(usize::from(padding).saturating_mul(2))
            .max(1)
    } else {
        width
    }
}

/// Paint the block layout over already-wrapped rows (issue #49): every row
/// becomes exactly `width` columns — `padding` background columns on each
/// side, content between them (hugging the right padding under
/// [`BlockAlign::Right`]) — and every span carries the background, so the
/// block reads as one solid bar. Content rows shorter than the block width
/// are filled with pure background spans; empty content rows render as pure
/// background. No background → no-op (classic flush layout).
fn apply_block_layout(
    rows: &mut [Line<'static>],
    width: usize,
    bg: Option<Color>,
    layout: &BlockTheme,
) {
    let Some(bg) = bg else { return };
    let width = width.max(1);
    let pad = usize::from(layout.padding).min(width / 2);
    let inner = width - pad * 2;
    for line in rows {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let content_w = unicode_width::UnicodeWidthStr::width(text.as_str());
        if content_w > inner {
            truncate_line(line, inner);
        }
        let gap = inner.saturating_sub(content_w.min(inner));
        let (lead, trail) = match layout.align {
            BlockAlign::Left => (pad, pad + gap),
            BlockAlign::Right => (pad + gap, pad),
        };
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
        if lead > 0 {
            spans.push(Span::styled(" ".repeat(lead), Style::new().bg(bg)));
        }
        for span in std::mem::take(&mut line.spans) {
            spans.push(Span::styled(span.content, span.style.bg(bg)));
        }
        if trail > 0 {
            spans.push(Span::styled(" ".repeat(trail), Style::new().bg(bg)));
        }
        line.spans = spans;
    }
}

/// Draw pre-wrapped lines into the visible window only (O(viewport)) — the
/// cache-friendly replacement for `Paragraph::new(lines).scroll(...)` (issue
/// #34). Rows outside the window are never touched, and the area is cleared
/// first so a shrinking feed cannot leave stale cells behind. `selection` is
/// a 2D feed selection in *capped* line coordinates (`(line, display
/// column)` pairs, issue #53): only the `[c1, c2)` column slice of each
/// selected row is painted with the selection background — never the whole
/// line, never the screen width.
pub fn render_lines_window(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    lines: &[Line<'static>],
    offset: usize,
    selection: Option<FeedSelection>,
) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.reset();
            }
        }
    }
    for (i, line) in lines.iter().enumerate().skip(offset) {
        let row = i - offset;
        if row >= area.height as usize {
            break;
        }
        let y = area.y + row as u16;
        let (c1, c2) = selection
            .as_ref()
            .map(|sel| sel.paint_cols(i, line))
            .unwrap_or((0, 0));
        if c1 < c2 {
            selection::highlight_cols(buf, area.x, y, line, c1, c2);
        } else {
            set_line_safe(buf, area.x, y, line, area.width);
        }
    }
}

/// Bounds-checked `Buffer::set_line`: resize races can leave a momentarily
/// out-of-bounds rect — skip the write instead of panicking.
fn set_line_safe(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, line: &Line<'_>, width: u16) {
    if y < buf.area.bottom() && x < buf.area.right() {
        buf.set_line(x, y, line, width);
    }
}

/// User rows, grok style: `❯ ` accent prefix + primary-colored body on a
/// full-width elevated band (the band color is the `user_bg` theme role);
/// continuation lines keep a 2-col indent.
fn push_user_block(out: &mut Vec<Line<'static>>, text: &str, width: usize, theme: &Theme) {
    for (i, para) in text.split('\n').enumerate() {
        let owned;
        let (prefix, body) = if i == 0 {
            (Some(USER_PREFIX), para)
        } else {
            (None, {
                owned = format!("{USER_BAND_INDENT}{para}");
                owned.as_str()
            })
        };
        let prefix_width = prefix
            .map(unicode_width::UnicodeWidthStr::width)
            .unwrap_or(0);
        let mut first = true;
        for row in wrap_str(body, width.saturating_sub(prefix_width).max(1)) {
            let mut spans = Vec::with_capacity(3);
            if first && let Some(prefix) = prefix {
                spans.push(Span::styled(prefix.to_string(), USER_STYLE));
            }
            spans.push(Span::styled(row.clone(), user_body_style(theme)));
            first = false;
            let row_width = if prefix.is_some() && spans.len() > 1 {
                prefix_width
            } else {
                0
            } + unicode_width::UnicodeWidthStr::width(row.as_str());
            let pad = width.saturating_sub(row_width);
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), band_style(theme)));
            }
            out.push(Line::from(spans));
        }
    }
}

/// Truncate a styled line to `max` display columns, dropping trailing spans
/// and slicing the final span (single-line tool rows never wrap).
fn truncate_line(line: &mut Line<'static>, max: usize) {
    let mut total = 0usize;
    let mut keep: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    for span in std::mem::take(&mut line.spans) {
        let w = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        if total + w <= max {
            total += w;
            keep.push(span);
        } else {
            let budget = max.saturating_sub(total);
            if budget == 0 {
                break;
            }
            let cut = theway_pager_render::line_utils::byte_offset_at_width(&span.content, budget);
            if let Some(s) = span.content.get(..cut).filter(|s| !s.is_empty()) {
                keep.push(Span::styled(s.to_string(), span.style));
            }
            break;
        }
    }
    line.spans = keep;
}

/// Human-format a count for stats lines: raw below 1000, otherwise `k` with
/// one decimal (`1.2k`, `100.2k`).
fn human_count(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

/// Build the thinking stats line for `chars` characters of thinking text:
/// char count (human format) on the left, `c/s` throughput + last-turn
/// input/output tokens on the right, right aligned to the content width.
/// Shared by the one-shot renderer and the streaming cache so both stay
/// byte-identical.
pub(crate) fn thinking_stats_line(
    chars: usize,
    opts: &FeedRenderOptions,
    width: usize,
) -> Line<'static> {
    let left = format!("{TOOL_PREFIX}thinking · {} char", human_count(chars as u64));
    let cps = opts.thinking_cps.round() as u64;
    let right = format!(
        "c/s: {} · in: {} · out: {}",
        cps,
        human_count(opts.thinking_input_tokens),
        human_count(opts.thinking_output_tokens)
    );
    let left_w = unicode_width::UnicodeWidthStr::width(left.as_str());
    let right_w = unicode_width::UnicodeWidthStr::width(right.as_str());
    let pad = width.saturating_sub(left_w + right_w).max(1);
    let mut line = Line::from(vec![
        Span::styled(left, RESULT_SUMMARY_STYLE),
        Span::styled(" ".repeat(pad), RESULT_SUMMARY_STYLE),
        Span::styled(right, RESULT_SUMMARY_STYLE),
    ]);
    truncate_line(&mut line, width);
    line
}

/// Push the thinking stats line for `text`. Hidden mode never renders it.
fn push_thinking_stats_line(
    out: &mut Vec<Line<'static>>,
    text: &str,
    opts: &FeedRenderOptions,
    width: usize,
) {
    out.push(thinking_stats_line(text.chars().count(), opts, width));
}

/// Thinking peek window: stats-line header, the last few wrapped lines, and a
/// mode hint.
fn push_thinking_peek(
    out: &mut Vec<Line<'static>>,
    text: &str,
    opts: &FeedRenderOptions,
    width: usize,
) {
    push_thinking_stats_line(out, text, opts, width);
    let wrapped: Vec<String> = text
        .split('\n')
        .flat_map(|para| wrap_str(para, width))
        .collect();
    let shown = wrapped.iter().rev().take(THINKING_PEEK_LINES).rev();
    for row in shown {
        out.push(Line::styled(
            format!("  {row}"),
            thinking_style(&opts.theme),
        ));
    }
    if wrapped.len() > THINKING_PEEK_LINES {
        out.push(Line::styled(
            "  … Ctrl+O cycles: hidden/peek/full",
            RESULT_SUMMARY_STYLE,
        ));
    }
}

/// Collapsed tool result: a bordered preview of the first
/// [`TOOL_RESULT_PREVIEW_LINES`] lines plus an `…(N more lines)` elision row
/// when the result is taller. Full expansion (Ctrl+T) keeps the whole body.
fn push_tool_result_preview(
    out: &mut Vec<Line<'static>>,
    lines: &[String],
    is_error: bool,
    width: usize,
    theme: &Theme,
) {
    let style = if is_error {
        Style::default().fg(theme.tool_error)
    } else {
        Style::default().fg(theme.tool_result)
    };
    let border_w = unicode_width::UnicodeWidthStr::width(TOOL_RESULT_BORDER);
    let content_w = width.saturating_sub(border_w).max(1);
    let preview_n = lines.len().min(TOOL_RESULT_PREVIEW_LINES);
    for line in &lines[..preview_n] {
        for row in wrap_str(line, content_w) {
            out.push(Line::styled(format!("{TOOL_RESULT_BORDER}{row}"), style));
        }
    }
    if lines.len() > TOOL_RESULT_PREVIEW_LINES {
        let more = lines.len() - TOOL_RESULT_PREVIEW_LINES;
        out.push(Line::styled(
            format!("{TOOL_RESULT_BORDER}…({more} more lines)"),
            RESULT_SUMMARY_STYLE,
        ));
    }
}

/// Expanded tool result (issue #41): non-fence lines render exactly as
/// before (`    `-indented, width-wrapped, result-colored); a ```mermaid
/// fence routes its body through the markdown mermaid render path into a
/// box-and-arrow diagram. The fence lines themselves are consumed by the
/// diagram, matching pretty-mode markdown.
fn push_tool_result_expanded(
    out: &mut Vec<Line<'static>>,
    lines: &[String],
    width: usize,
    style: Style,
) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().starts_with("```mermaid") {
            let mut body: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().starts_with("```") {
                body.push(lines[j].as_str());
                j += 1;
            }
            // `j` is the closing fence index, or `lines.len()` when unclosed
            // (the rest of the block is the body, like a truncated fence).
            push_mermaid_diagram(out, &body.join("\n"), style, width);
            i = j + 1;
            continue;
        }
        for row in wrap_str(&format!("    {}", lines[i]), width) {
            out.push(Line::styled(row, style));
        }
        i += 1;
    }
}

/// Render a mermaid body through [`theway_markdown::render_mermaid_art`] and
/// push the diagram lines, recolored to the tool result `style`; blank
/// bodies fall back to the ordinary text rows. Over-wide or unsupported
/// sources render as the renderer's framed source box (markdown parity).
fn push_mermaid_diagram(out: &mut Vec<Line<'static>>, src: &str, style: Style, width: usize) {
    let Some(mut art) = theway_markdown::render_mermaid_art(
        src,
        &theway_markdown::MermaidStyles::default(),
        Some(width),
    ) else {
        for row in wrap_str(src, width) {
            out.push(Line::styled(row, style));
        }
        return;
    };
    if let Some(fg) = style.fg {
        for line in &mut art.styled_lines {
            for span in &mut line.spans {
                if span.style.fg.is_none() {
                    span.style = span.style.fg(fg);
                }
            }
        }
    }
    out.extend(art.styled_lines);
}

/// Stateful, width-aware incremental wrapper with the exact `wrap_str`
/// semantics (break at last space, hard-break overlong words, preserve
/// leading whitespace) applied across arbitrary chunk boundaries (issue #35).
///
/// `push_str` feeds appended text and moves COMPLETE rows into `rows`; the
/// current partial row stays in `tail`. A `\n` always terminates the current
/// row (empty paragraphs yield an empty row, matching `push_paragraphs`).
pub(crate) struct IncrementalWrap {
    width: usize,
    /// Current partial row (the live tail line).
    pub(crate) tail: String,
    /// Complete rows flushed so far.
    pub rows: Vec<String>,
}

impl IncrementalWrap {
    pub fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            tail: String::new(),
            rows: Vec::new(),
        }
    }

    /// Append text; flushes every row completed by the append.
    pub fn push_str(&mut self, delta: &str) {
        for ch in delta.chars() {
            if ch == '\n' {
                self.rows.push(std::mem::take(&mut self.tail));
                continue;
            }
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let cur_w = unicode_width::UnicodeWidthStr::width(self.tail.as_str());
            if cur_w + cw > self.width && !self.tail.is_empty() {
                if let Some(bp) = self.tail.rfind(' ') {
                    let rest = self.tail.split_off(bp);
                    let rest = rest.trim_start_matches(' ').to_string();
                    self.rows.push(self.tail.trim_end().to_string());
                    self.tail = rest;
                } else {
                    self.rows.push(std::mem::take(&mut self.tail));
                }
            }
            self.tail.push(ch);
        }
    }
}

/// Underline URLs detected in the rendered lines (theway-pager-render osc8
/// detection). Actual OSC 8 hyperlink sequence output needs the inline
/// terminal (Phase 4, `theway-ratatui-inline`); until then URLs get the
/// underline affordance only.
///
/// Assistant rows are skipped: their underlines come from the markdown
/// renderer's `hyperlinks` output instead of this regex scan.
fn underline_links(lines: &mut [Line<'static>], skip: &[std::ops::Range<usize>]) {
    use theway_pager_render::osc8::{LinkOverlay, scan_lines_for_url_overlays};
    let mut overlay = LinkOverlay::new();
    scan_lines_for_url_overlays(
        lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i as u16, line, None)),
        0,
        &[],
        &mut overlay,
    );
    for link in overlay.links() {
        let row = link.screen_row as usize;
        if skip.iter().any(|range| range.contains(&row)) {
            continue;
        }
        if let Some(line) = lines.get_mut(row) {
            underline_range(line, link.col_start as usize, link.col_end as usize);
        }
    }
}

/// Split a line's spans so the display-column range `[start_col, end_col)`
/// carries the underline modifier; the rest of the line is untouched.
fn underline_range(line: &mut Line<'static>, start_col: usize, end_col: usize) {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    for span in std::mem::take(&mut line.spans) {
        let width = unicode_width::UnicodeWidthStr::width(span.content.as_ref());
        let span_end = col + width;
        if span_end <= start_col || col >= end_col {
            out.push(span);
        } else {
            let cut_start = start_col.max(col) - col;
            let cut_end = end_col.min(span_end) - col;
            let pre = span
                .content
                .get(
                    ..theway_pager_render::line_utils::byte_offset_at_width(
                        &span.content,
                        cut_start,
                    ),
                )
                .filter(|s| !s.is_empty())
                .map(|s| Span::styled(s.to_string(), span.style));
            let mid = span
                .content
                .get(
                    theway_pager_render::line_utils::byte_offset_at_width(&span.content, cut_start)
                        ..theway_pager_render::line_utils::byte_offset_at_width(
                            &span.content,
                            cut_end,
                        ),
                )
                .map(|s| {
                    Span::styled(s.to_string(), span.style.add_modifier(Modifier::UNDERLINED))
                });
            let post = span
                .content
                .get(
                    theway_pager_render::line_utils::byte_offset_at_width(&span.content, cut_end)..,
                )
                .filter(|s| !s.is_empty())
                .map(|s| Span::styled(s.to_string(), span.style));
            if let Some(p) = pre {
                out.push(p);
            }
            if let Some(m) = mid {
                out.push(m);
            }
            if let Some(p) = post {
                out.push(p);
            }
        }
        col = span_end;
    }
    line.spans = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use theway_transport::feed::{Feed, WireFeedBlock};
    use unicode_width::UnicodeWidthStr;

    fn feed_with(blocks: &[WireFeedBlock]) -> Feed {
        let mut feed = Feed::new();
        feed.replace_blocks(blocks);
        feed
    }

    fn flat(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn user_block_has_accent_prefix_and_band() {
        let feed = feed_with(&[WireFeedBlock::User {
            text: "hello world".into(),
            timestamp: Some("2026-01-01 12:00".into()),
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 30, &opts);
        assert_eq!(lines.len(), 1);
        let spans = &lines[0].spans;
        assert_eq!(spans[0].content, "\u{276f} ");
        assert_eq!(spans[0].style.fg, Some(ACCENT_USER));
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[1].content, "hello world");
        // Trailing band span pads the row to full width.
        let total: usize = spans.iter().map(|s| s.content.width()).sum();
        assert_eq!(total, 30);
        assert_eq!(spans[2].style.bg, Some(BG_HIGHLIGHT));
        // Timestamps are dropped from conversational blocks.
        assert!(!flat(&lines).contains("2026-01-01"));
    }

    #[test]
    fn user_block_wraps_with_indent_and_band() {
        let feed = feed_with(&[WireFeedBlock::User {
            text: "one two three four five six seven".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 12, &opts);
        assert!(lines.len() >= 2, "expected wrap: {lines:?}");
        // Continuation rows keep the band width.
        for line in &lines {
            let total: usize = line.spans.iter().map(|s| s.content.width()).sum();
            assert_eq!(total, 12);
        }
    }

    #[test]
    fn thinking_modes_full_peek_hidden() {
        let blocks = vec![
            WireFeedBlock::User {
                text: "go".into(),
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "deep thoughts about the plan".into(),
                timestamp: None,
            },
        ];
        let opts = |mode| FeedRenderOptions {
            thinking_mode: mode,
            tools_expanded: false,
            ..Default::default()
        };
        let full = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Full),
        ));
        assert!(full.contains("deep thoughts"), "{full}");
        let peek = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Peek),
        ));
        assert!(peek.contains("⏵ thinking · 28 char"), "{peek}");
        assert!(peek.contains("c/s: 0 · in: 0 · out: 0"), "{peek}");
        assert!(peek.contains("deep thoughts"), "{peek}");
        let hidden = flat(&super::lines(
            &feed_with(&blocks),
            80,
            &opts(ThinkingMode::Hidden),
        ));
        assert!(!hidden.contains("deep thoughts"), "{hidden}");
        assert!(hidden.contains("❯ go"), "{hidden}");
    }

    #[test]
    fn thinking_peek_windows_tail_lines() {
        let text = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Peek,
            tools_expanded: false,
            ..Default::default()
        };
        let lines = super::lines(&feed, 80, &opts);
        // Header + 3 peek rows + mode hint.
        assert!(lines.len() <= 1 + THINKING_PEEK_LINES + 1, "{lines:?}");
        let flat = flat(&lines);
        assert!(flat.contains("line 27"), "{flat}");
        assert!(flat.contains("line 29"), "{flat}");
        assert!(!flat.contains("line 0"), "{flat}");
    }

    #[test]
    fn tool_call_is_single_accent_line_without_timestamp() {
        let feed = feed_with(&[WireFeedBlock::Tool {
            name: "read".into(),
            args: "(path=\"x\")".into(),
            timestamp: Some("2026-01-01 12:00".into()),
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].content, "\u{23f5} read");
        assert_eq!(lines[0].spans[0].style.fg, Some(ACCENT_TOOL));
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[1].content.contains("(path=\"x\")"));
        assert!(!flat(&lines).contains("2026-01-01"));
    }

    #[test]
    fn tool_result_collapses_to_preview_and_expands() {
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines: vec!["line a".into(), "line b".into()],
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        assert!(collapsed.contains("│ line a"), "{collapsed}");
        assert!(collapsed.contains("│ line b"), "{collapsed}");
        assert!(!collapsed.contains("more lines"), "{collapsed}");
        let expanded = flat(&super::lines(
            &feed,
            80,
            &FeedRenderOptions {
                thinking_mode: ThinkingMode::Full,
                tools_expanded: true,
                ..Default::default()
            },
        ));
        assert!(expanded.contains("    line a"), "{expanded}");
        assert!(expanded.contains("    line b"), "{expanded}");
    }

    #[test]
    fn tool_result_preview_folds_to_five_lines_with_elision() {
        let lines: Vec<String> = (0..8).map(|i| format!("row {i}")).collect();
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        // The first 5 source lines render, each behind the left border.
        for i in 0..5 {
            assert!(collapsed.contains(&format!("│ row {i}")), "{collapsed}");
        }
        // Lines beyond the preview window are folded away.
        assert!(!collapsed.contains("row 5"), "{collapsed}");
        assert!(!collapsed.contains("row 7"), "{collapsed}");
        // The elision row carries the remaining line count.
        assert!(collapsed.contains("…(3 more lines)"), "{collapsed}");
    }

    #[test]
    fn tool_result_expanded_keeps_all_lines() {
        let lines: Vec<String> = (0..8).map(|i| format!("row {i}")).collect();
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            tools_expanded: true,
            ..Default::default()
        };
        let expanded = flat(&super::lines(&feed, 80, &opts));
        for i in 0..8 {
            assert!(expanded.contains(&format!("row {i}")), "{expanded}");
        }
        assert!(!expanded.contains("more lines"), "{expanded}");
    }

    /// Issue #41: an expanded tool result containing a ```mermaid fence
    /// renders the fenced body as box-drawing diagram art; non-fence lines
    /// keep the existing indented text rows.
    #[test]
    fn tool_result_mermaid_fence_renders_box_diagram() {
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines: vec![
                "before".into(),
                "```mermaid".into(),
                "graph TD".into(),
                "  A[Start] --> B[End]".into(),
                "```".into(),
                "after".into(),
            ],
            is_error: false,
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            tools_expanded: true,
            ..Default::default()
        };
        let expanded = flat(&super::lines(&feed, 80, &opts));
        assert!(
            expanded.chars().any(|c| "┌┐─".contains(c)),
            "expected mermaid box art: {expanded}"
        );
        // Non-fence lines stay as today (indented text rows).
        assert!(expanded.contains("    before"), "{expanded}");
        assert!(expanded.contains("    after"), "{expanded}");
        // The fence delimiters are consumed by the diagram.
        assert!(!expanded.contains("```"), "{expanded}");
    }

    /// Only the expanded branch detects fences: the collapsed preview keeps
    /// the classic text rendering (the fence stays visible behind the
    /// preview border).
    #[test]
    fn tool_result_collapsed_preview_keeps_fence_text() {
        let lines = vec![
            "```mermaid".into(),
            "graph TD".into(),
            "  A --> B".into(),
            "```".into(),
        ];
        let feed = feed_with(&[WireFeedBlock::ToolResult {
            lines,
            is_error: false,
            timestamp: None,
        }]);
        let collapsed = flat(&super::lines(&feed, 80, &FeedRenderOptions::default()));
        assert!(
            !collapsed.chars().any(|c| "┌┐─".contains(c)),
            "collapsed preview must not render the diagram: {collapsed}"
        );
        let expanded = flat(&super::lines(
            &feed,
            80,
            &FeedRenderOptions {
                tools_expanded: true,
                ..Default::default()
            },
        ));
        assert!(
            expanded.chars().any(|c| "┌┐─".contains(c)),
            "expanded fence must render the diagram: {expanded}"
        );
    }

    #[test]
    fn human_count_formats_raw_and_kilo() {
        assert_eq!(human_count(0), "0");
        assert_eq!(human_count(999), "999");
        assert_eq!(human_count(1000), "1.0k");
        assert_eq!(human_count(1200), "1.2k");
        assert_eq!(human_count(1234), "1.2k");
        assert_eq!(human_count(100_200), "100.2k");
    }

    #[test]
    fn thinking_hidden_renders_no_stats() {
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text: "some thinking text".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Hidden,
            thinking_cps: 84.0,
            thinking_output_tokens: 1200,
            ..Default::default()
        };
        let flat = flat(&super::lines(&feed, 80, &opts));
        assert!(!flat.contains("c/s"), "{flat}");
        assert!(!flat.contains("thinking"), "{flat}");
        assert!(!flat.contains("some thinking text"), "{flat}");
    }

    #[test]
    fn thinking_stats_line_formats_cps_and_in_out_tokens() {
        let feed = feed_with(&[WireFeedBlock::Thinking {
            text: "x".repeat(1200),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions {
            thinking_mode: ThinkingMode::Full,
            thinking_cps: 84.0,
            thinking_input_tokens: 57_100,
            thinking_output_tokens: 1_200,
            ..Default::default()
        };
        let flat = flat(&super::lines(&feed, 80, &opts));
        assert!(flat.contains("⏵ thinking · 1.2k char"), "{flat}");
        assert!(flat.contains("c/s: 84 · in: 57.1k · out: 1.2k"), "{flat}");
    }

    #[test]
    fn feed_render_options_defaults() {
        let opts = FeedRenderOptions::default();
        assert_eq!(opts.thinking_mode, ThinkingMode::default());
        assert!(!opts.tools_expanded);
        assert_eq!(opts.thinking_cps, 0.0);
        assert_eq!(opts.thinking_input_tokens, 0);
        assert_eq!(opts.thinking_output_tokens, 0);
        assert_eq!(opts.spinner_phase, 0);
    }

    /// `PartialEq` is hand-implemented (issue #44): the per-frame counters
    /// (cps / in / out / spinner_phase) must NOT participate, otherwise the
    /// feed cache invalidates and fully re-renders every frame; structural
    /// switches (thinking_mode / tools_expanded / theme) must.
    #[test]
    fn feed_render_options_equality_ignores_per_frame_counters() {
        let structural = FeedRenderOptions::default();
        let mut per_frame = FeedRenderOptions::default();
        per_frame.thinking_cps = 999.5;
        per_frame.thinking_input_tokens = 57_100;
        per_frame.thinking_output_tokens = 1_200;
        per_frame.spinner_phase = 42;
        assert_eq!(
            structural, per_frame,
            "cps/in/out/spinner_phase changes must keep options equal"
        );
        per_frame.thinking_mode = ThinkingMode::Peek;
        assert_ne!(
            structural, per_frame,
            "thinking_mode change must change equality"
        );
        per_frame = FeedRenderOptions::default();
        per_frame.tools_expanded = true;
        assert_ne!(
            structural, per_frame,
            "tools_expanded change must change equality"
        );
        per_frame = FeedRenderOptions::default();
        per_frame.theme.tool_title = Color::Rgb(1, 2, 3);
        assert_ne!(
            structural, per_frame,
            "theme change must change equality (issue #49: the cache fingerprints colors/layout)"
        );
    }

    /// Custom theme block layout (issue #49): the tool row carries the
    /// `tool_running_bg` background across the FULL block width with the
    /// configured padding columns, right-aligned.
    #[test]
    fn custom_theme_paints_tool_row_bg_padding_and_right_align() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool_running_bg = Some(Color::Rgb(1, 2, 3));
        theme.tool.padding = 2;
        theme.tool.align = crate::ui::theme::BlockAlign::Right;
        let opts = FeedRenderOptions {
            theme,
            ..Default::default()
        };
        let feed = feed_with(&[WireFeedBlock::Tool {
            name: "read".into(),
            args: String::new(),
            timestamp: None,
        }]);
        let lines = super::lines(&feed, 20, &opts);
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.trim(), "⏵ read", "{text}");
        // Right padding columns: content ends 2 columns before the edge.
        assert!(text.ends_with("  "), "{text}");
        // Every span (content + padding) carries the background.
        for span in &lines[0].spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(1, 2, 3)));
        }
        // The row fills the whole block width.
        let total: usize = lines[0]
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20);
    }

    /// Custom theme thinking layout (issue #49): stats line AND body rows get
    /// the `thinking_bg` background at full width; an empty result row renders
    /// as pure background.
    #[test]
    fn custom_theme_paints_thinking_and_empty_result_rows() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.thinking_bg = Some(Color::Rgb(4, 5, 6));
        theme.thinking.padding = 1;
        theme.thinking.align = crate::ui::theme::BlockAlign::Left;
        theme.tool_success_bg = Some(Color::Rgb(7, 8, 9));
        let opts = FeedRenderOptions {
            theme,
            tools_expanded: true,
            ..Default::default()
        };
        let feed = feed_with(&[
            WireFeedBlock::Thinking {
                text: "ponder".into(),
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["row".into(), String::new()],
                is_error: false,
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 20, &opts);
        let flat = flat(&lines);
        // Thinking stats row + body row both painted at full width.
        let stats = &lines[0];
        assert!(flat.contains("⏵ thinking"), "{flat}");
        for span in &stats.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(4, 5, 6)));
        }
        let total: usize = stats
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20, "stats row must span the full block width");
        let body = &lines[1];
        for span in &body.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(4, 5, 6)));
        }
        // The expanded empty result line renders as a pure-background row.
        // Layout: [separator? no — blocks 0..1 thinking, then result rows]
        // Find the empty result row: after the thinking rows + separator.
        let empty = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .all(|s| s.content.chars().all(|c| c == ' '))
                    && !line.spans.is_empty()
            })
            .expect("empty result row missing");
        for span in &empty.spans {
            assert_eq!(span.style.bg, Some(Color::Rgb(7, 8, 9)));
        }
        let total: usize = empty
            .spans
            .iter()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(total, 20, "empty row must be pure background at full width");
    }

    /// Theme role colors (issue #43): tool result / error colors flow from
    /// the theme; the default theme equals the pre-theme consts.
    #[test]
    fn custom_theme_recolors_tool_result_and_error() {
        let mut theme = crate::ui::theme::Theme::default();
        theme.tool_result = Color::Rgb(1, 2, 3);
        theme.tool_error = Color::Rgb(4, 5, 6);
        let opts = FeedRenderOptions {
            theme,
            tools_expanded: true,
            ..Default::default()
        };
        let feed = feed_with(&[
            WireFeedBlock::ToolResult {
                lines: vec!["ok".into()],
                is_error: false,
                timestamp: None,
            },
            WireFeedBlock::ToolResult {
                lines: vec!["bad".into()],
                is_error: true,
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 80, &opts);
        let ok = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("ok")))
            .unwrap();
        let bad = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content.contains("bad")))
            .unwrap();
        // `Line::styled` puts the fg on the line itself (span styles stay
        // default) — assert the effective line style.
        assert_eq!(ok.style.fg, Some(Color::Rgb(1, 2, 3)), "{ok:?}");
        assert_eq!(bad.style.fg, Some(Color::Rgb(4, 5, 6)), "{bad:?}");
    }

    /// Default theme renders byte-identical rows to the pre-theme consts:
    /// no background, no padding columns, flush left (issue #49).
    #[test]
    fn default_theme_keeps_classic_tool_and_thinking_rows() {
        let feed = feed_with(&[
            WireFeedBlock::Tool {
                name: "read".into(),
                args: "(path=\"x\")".into(),
                timestamp: None,
            },
            WireFeedBlock::Thinking {
                text: "ponder".into(),
                timestamp: None,
            },
        ]);
        let lines = super::lines(&feed, 20, &FeedRenderOptions::default());
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "⏵ read (path=\"x\")", "tool row must stay flush");
        assert!(lines[0].spans.iter().all(|s| s.style.bg.is_none()));
        let stats: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(stats.contains("⏵ thinking"), "{stats}");
        assert!(lines[1].spans.iter().all(|s| s.style.bg.is_none()));
    }

    #[test]
    fn mermaid_fence_renders_diagram_not_source() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "```mermaid\ngraph TD\n  A[Start] --> B[End]\n```\n".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        let flat = flat(&lines);
        assert!(
            flat.chars().any(|c| "─│┌┐└┘├┤┬┴┼".contains(c)),
            "expected diagram art: {flat}"
        );
        assert!(!flat.contains("graph TD"), "{flat}");
    }

    #[test]
    fn assistant_prefix_is_magenta() {
        let feed = feed_with(&[WireFeedBlock::Assistant {
            text: "plain answer".into(),
            timestamp: None,
        }]);
        let opts = FeedRenderOptions::default();
        let lines = super::lines(&feed, 80, &opts);
        assert!(lines[0].spans[0].content.starts_with("ai ▸ "));
        assert_eq!(lines[0].spans[0].style.fg, Some(ACCENT_ASSISTANT));
    }

    /// `wrap_str_ranges` must produce rows identical to the transport
    /// `wrap_str` (byte ranges re-slice the source to the same rows).
    #[test]
    fn wrap_str_ranges_matches_wrap_str() {
        let cases: Vec<String> = vec![
            String::new(),
            "hello".to_string(),
            "hello world".to_string(),
            "aa bb cc dd ee".to_string(),
            "word ".repeat(30),
            "https://example.com/very/long/path".to_string(),
            "  leading spaces preserved".to_string(),
            "mix of 中文 and ascii text".to_string(),
        ];
        for text in &cases {
            for width in [1usize, 2, 5, 8, 20, 80] {
                let expected = theway_transport::feed::wrap_str(text, width);
                let got: Vec<String> = wrap_str_ranges(text, width)
                    .into_iter()
                    .map(|row| text[row.range].to_string())
                    .collect();
                assert_eq!(got, expected, "text={text:?} width={width}");
            }
        }
    }
}

#[cfg(test)]
mod wrap_property_tests {
    use super::IncrementalWrap;
    use theway_transport::feed::wrap_str;

    #[test]
    fn incremental_wrap_matches_wrap_str_across_chunk_boundaries() {
        let texts = [
            "hello world",
            "aa bb cc dd ee ff",
            "  leading spaces preserved",
            "mix of 中文 and ascii text",
            "one\ntwo\n\nfour",
            "",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];
        // Long repeating-word text constructed dynamically.
        let long_words = "word ".repeat(30);
        let texts = texts
            .into_iter()
            .chain(std::iter::once(long_words.as_str()))
            .collect::<Vec<_>>();
        for text in texts {
            for width in [1usize, 3, 6, 12, 20] {
                let expected: Vec<String> = text
                    .split('\n')
                    .flat_map(|para| wrap_str(para, width))
                    .collect();
                // Push in 1-, 2- and 3-char chunks; every boundary must agree.
                for step in [1usize, 2, 3] {
                    let mut wrap = IncrementalWrap::new(width);
                    let mut offset = 0;
                    while offset < text.len() {
                        let mut end = (offset + step).min(text.len());
                        while end < text.len() && !text.is_char_boundary(end) {
                            end += 1;
                        }
                        wrap.push_str(&text[offset..end]);
                        offset = end;
                    }
                    let mut got = wrap.rows;
                    got.push(wrap.tail.clone());
                    assert_eq!(got, expected, "text={text:?} width={width} step={step}");
                }
            }
        }
    }
}
