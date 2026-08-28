//! Ratatui rendering for the conversation feed — the terminal rendering lives
//! here in the TUI; the UI-agnostic model lives in `theway_transport::feed`.
//!
//! [`lines`] renders a [`Feed`] to width-wrapped, styled `ratatui` lines,
//! ready to scroll/draw — the terminal counterpart of `Feed::plain_lines`.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use theway_transport::feed::{Block, Level, display_prefix, wrap_str};

use crate::ui::theme::{BlockAlign, BlockBorder, BlockTheme, Theme};

/// Grok tokyonight palette values (xai-grok-pager-render theme/tokyonight.rs).
const ACCENT_USER: Color = Color::Rgb(122, 162, 247); // BLUE — user `❯` prefix
const ACCENT_ASSISTANT: Color = Color::Rgb(187, 154, 247); // legacy theme role
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
pub(crate) const TOOL_PREFIX: &str = "\u{23f5} "; // ⏵
const USER_BAND_INDENT: &str = "  ";

const USER_STYLE: Style = Style::new().fg(ACCENT_USER).add_modifier(Modifier::BOLD);
/// Default thinking style; the streaming thinking path in `feed_cache`
/// reuses this const (theme-aware colors apply to one-shot renders).
pub(crate) const THINKING_STYLE: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
pub(crate) const RESULT_SUMMARY_STYLE: Style = Style::new().fg(Color::DarkGray);

fn user_body_style(theme: &Theme) -> Style {
    Style::new().fg(theme.user_text)
}
fn band_style(theme: &Theme) -> Style {
    Style::new().bg(theme.user_bg)
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
    /// Terminal capability resolved by the owning client. Tests use the
    /// `TrueColor` default instead of ambient process environment state.
    pub color_level: theway_markdown::ColorLevel,
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
            && self.color_level == other.color_level
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
pub(crate) fn markdown_style(
    color_level: theway_markdown::ColorLevel,
) -> theway_markdown::MarkdownStyle {
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
    .adapt_for(color_level)
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
/// A non-empty `prefix` is prepended to the first rendered line only, in
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
    color_level: theway_markdown::ColorLevel,
) {
    let (rendered, _checkpoint) = theway_markdown::render_markdown_ratatui_full_width(
        text,
        markdown_style(color_level),
        true,
        Some(theway_markdown::default_syntect_with_color_level(
            color_level,
        )),
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
        if first && !prefix.is_empty() {
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
        if first && !prefix.is_empty() {
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

/// Push the inter-block separator rows (issue #30): `gap` blank lines, or a
/// full-width styled separator line when `[feed] separator` is set (the gap
/// still applies above it). Callers decide WHERE a gap goes via the transport
/// `should_separate`; this decides HOW MUCH.
pub(crate) fn push_feed_gap(
    out: &mut Vec<Line<'static>>,
    width: usize,
    feed: &crate::ui::theme::FeedTheme,
) {
    for _ in 0..feed.gap {
        out.push(Line::raw(""));
    }
    if let Some(glyph) = feed.separator {
        let line = glyph.to_string().repeat(width.max(1));
        out.push(Line::from(Span::styled(
            line,
            Style::default().fg(feed.separator_style),
        )));
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
        if theway_transport::feed::should_separate_with(
            previous,
            block,
            !out.is_empty(),
            opts.theme.feed.separate_all,
        ) {
            push_feed_gap(&mut out, width, &opts.theme.feed);
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
            push_markdown(
                &mut out,
                text,
                "",
                Style::default(),
                width,
                opts.color_level,
            );
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
            apply_block_frame(&mut rows, width, &theme.thinking);
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
            apply_block_frame(&mut rows, width, &theme.tool);
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
            apply_block_frame(&mut rows, width, &theme.tool);
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

/// Apply the block frame (issue #31): `margin_top` blank rows, a top border
/// line, the content, a bottom border line, then `margin_bottom` blank rows.
/// Margins accumulate with `[feed] gap` (emitted separately by
/// `push_feed_gap`); borders draw inside the margins. A no-op when every
/// frame knob is at its default, so default rendering stays byte-identical.
fn apply_block_frame(rows: &mut Vec<Line<'static>>, width: usize, layout: &BlockTheme) {
    if layout.margin_top == 0
        && layout.margin_bottom == 0
        && layout.border_top == BlockBorder::None
        && layout.border_bottom == BlockBorder::None
    {
        return;
    }
    let mut framed: Vec<Line<'static>> = Vec::with_capacity(rows.len() + 4);
    for _ in 0..layout.margin_top {
        framed.push(Line::raw(""));
    }
    if let Some(glyph) = border_glyph(layout.border_top) {
        framed.push(border_line(width, glyph, layout.border_style));
    }
    framed.extend(std::mem::take(rows));
    if let Some(glyph) = border_glyph(layout.border_bottom) {
        framed.push(border_line(width, glyph, layout.border_style));
    }
    for _ in 0..layout.margin_bottom {
        framed.push(Line::raw(""));
    }
    *rows = framed;
}

fn border_glyph(border: BlockBorder) -> Option<char> {
    match border {
        BlockBorder::None => None,
        BlockBorder::Thin => Some('─'),
        BlockBorder::Thick => Some('━'),
    }
}

fn border_line(width: usize, glyph: char, style: Color) -> Line<'static> {
    Line::from(Span::styled(
        glyph.to_string().repeat(width.max(1)),
        Style::default().fg(style),
    ))
}

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/window.rs"
));

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/feed_render/incremental_wrap.rs"
));
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
tests_bridge_macro::tests_bridge!("feed_render/unit");

#[cfg(test)]
mod feed_render_property_tests {
    tests_bridge_macro::tests_bridge!("feed_render/properties");
}
