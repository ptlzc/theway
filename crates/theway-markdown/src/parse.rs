//! Markdown parser - transforms markdown text into styled highlight ranges.
//!
//! The parser processes markdown events and populates buffers with:
//! - Highlights: Style ranges for inline formatting
//! - Replaces: Syntax-highlighted code blocks
//! - Transforms: Character substitutions (bullets, etc.)
//! - Table replaces: Formatted table content

use std::ops::Range;

use anstyle::Style;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Tag, TagEnd, TextMergeWithOffset};
use ratatui::style::Stylize as RatatuiStylize;
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

use crate::buffers::{
    CodeBlockMeta, Highlight, LinkTarget, MarkdownBuffers, Replace, StyledCell, TableHyperlink,
    TableReplace, TableState, Transform, floor_char_boundary, unicode_display_width,
};
use crate::checkpoint::CheckpointKind;
use crate::latex;
use crate::open_code_highlighter::OpenCodeHighlighter;
use crate::style::{MarkdownStyle, TableBorders};
use crate::syntax::{Syntect, syntax_highlight_raw};

/// Trait for converting anstyle to ratatui style.
trait StyleInto<T> {
    fn style_into(self) -> T;
}

impl StyleInto<ratatui::style::Style> for Style {
    fn style_into(self) -> ratatui::style::Style {
        use ratatui::style::{Modifier, Style as RStyle};

        let mut style = RStyle::default();

        if let Some(fg) = self.get_fg_color() {
            style = style.fg(anstyle_to_ratatui_color(fg));
        }
        if let Some(bg) = self.get_bg_color() {
            style = style.bg(anstyle_to_ratatui_color(bg));
        }

        let effects = self.get_effects();
        let mut modifiers = Modifier::empty();
        if effects.contains(anstyle::Effects::BOLD) {
            modifiers |= Modifier::BOLD;
        }
        if effects.contains(anstyle::Effects::DIMMED) {
            modifiers |= Modifier::DIM;
        }
        if effects.contains(anstyle::Effects::ITALIC) {
            modifiers |= Modifier::ITALIC;
        }
        if effects.contains(anstyle::Effects::UNDERLINE) {
            modifiers |= Modifier::UNDERLINED;
        }
        if effects.contains(anstyle::Effects::STRIKETHROUGH) {
            modifiers |= Modifier::CROSSED_OUT;
        }
        if effects.contains(anstyle::Effects::HIDDEN) {
            modifiers |= Modifier::HIDDEN;
        }

        style.add_modifier(modifiers)
    }
}

fn anstyle_to_ratatui_color(color: anstyle::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        anstyle::Color::Ansi(ansi) => match ansi {
            anstyle::AnsiColor::Black => Color::Black,
            anstyle::AnsiColor::Red => Color::Red,
            anstyle::AnsiColor::Green => Color::Green,
            anstyle::AnsiColor::Yellow => Color::Yellow,
            anstyle::AnsiColor::Blue => Color::Blue,
            anstyle::AnsiColor::Magenta => Color::Magenta,
            anstyle::AnsiColor::Cyan => Color::Cyan,
            anstyle::AnsiColor::White => Color::Gray,
            anstyle::AnsiColor::BrightBlack => Color::DarkGray,
            anstyle::AnsiColor::BrightRed => Color::LightRed,
            anstyle::AnsiColor::BrightGreen => Color::LightGreen,
            anstyle::AnsiColor::BrightYellow => Color::LightYellow,
            anstyle::AnsiColor::BrightBlue => Color::LightBlue,
            anstyle::AnsiColor::BrightMagenta => Color::LightMagenta,
            anstyle::AnsiColor::BrightCyan => Color::LightCyan,
            anstyle::AnsiColor::BrightWhite => Color::White,
        },
        anstyle::Color::Ansi256(idx) => Color::Indexed(idx.index()),
        anstyle::Color::Rgb(rgb) => Color::Rgb(rgb.0, rgb.1, rgb.2),
    }
}

/// Find a substring within a haystack, optionally searching outside or using rfind.
fn find_substring(
    haystack: &str,
    needle: &CowStr,
    allow_outside: bool,
    rfind: bool,
) -> Option<Range<usize>> {
    if needle.is_empty() || haystack.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    if !allow_outside {
        if let CowStr::Borrowed(needle) = needle {
            let (hp, np) = (haystack.as_ptr(), needle.as_ptr());
            unsafe {
                let (he, ne) = (hp.add(haystack.len()), np.add(needle.len()));
                if np >= hp && ne <= he {
                    let offset = np.offset_from(hp) as usize;
                    let range = offset..(offset + needle.len());
                    if cfg!(debug_assertions) {
                        assert_eq!(&haystack.as_bytes()[range.clone()], needle.as_bytes());
                    }
                    return Some(range);
                }
            }
        }
        None
    } else {
        if rfind {
            haystack.rfind(needle.as_ref())
        } else {
            haystack.find(needle.as_ref())
        }
        .map(|pos| pos..(pos + needle.len()))
    }
}

/// Decode a single HTML character entity reference (`entity` includes the
/// leading `&` and trailing `;`) into its replacement string.
///
/// Delegates to [`html_escape`] for the full HTML5 named set plus numeric
/// references (decimal `&#NN;` and hexadecimal `&#xNN;`), matching what
/// pulldown-cmark decodes in table cells so prose and tables stay consistent.
///
/// Returns `None` when:
/// - the reference is unrecognized (`html_escape` leaves it unchanged), or
/// - it decodes to a control character (`&#27;`, `&#0;`, …). Substituting raw
///   control bytes would let untrusted markdown inject terminal escape
///   sequences, so the raw source is left literal instead.
fn decode_html_entity(entity: &str) -> Option<String> {
    let decoded = html_escape::decode_html_entities(entity);
    // Unchanged output means `html_escape` did not recognize the reference.
    if decoded.as_ref() == entity {
        return None;
    }
    if decoded.chars().any(char::is_control) {
        return None;
    }
    Some(decoded.into_owned())
}

/// Check if there's a blank line (empty line) after the given position.
fn has_blank_line_after(text: &str, pos: usize) -> bool {
    text.as_bytes()[pos..]
        .iter()
        .copied()
        .find(|&c| c != b' ' && c != b'\t')
        == Some(b'\n')
}

/// Transient state for the fenced code block currently being parsed.
///
/// Fenced blocks never nest (an inner fence closes the outer), so a single
/// `Option` suffices. Finalized in the `TagEnd::CodeBlock` arm, where the body
/// range and the block range together decide whether the fence was closed.
struct PendingCodeBlock {
    info: String,
    /// Body byte range in the raw source. Initialized to an empty range just
    /// past the opening fence line, then widened to the merged body text range
    /// as text events arrive (`body_seen` distinguishes the empty-body case).
    body_range: Range<usize>,
    /// De-prefixed body content: pulldown's merged text gives the logical code
    /// with container markers (blockquote `>`, list indent) stripped and CRLF
    /// normalized to `\n` — i.e. the clean diagram/code source.
    body_text: String,
    body_seen: bool,
}

/// Markdown parser that processes events and populates buffers.
///
/// After calling `parse()`, the transient state (tag_stack, table_state, depth)
/// is dropped and a `ParsedMarkdown` is returned for rendering.
pub struct MarkdownParser<'a, 'b, 'syn, 'oc> {
    text: &'a str,
    ms: MarkdownStyle,
    buffers: &'b mut MarkdownBuffers,
    syntect: Option<&'syn Syntect>,
    /// Incremental highlighter for the trailing still-open fenced code block.
    /// Only set by the streaming tail re-render; `None` for batch renders, in
    /// which case code blocks go through the from-scratch [`syntax_highlight_raw`].
    open_code: Option<&'oc mut OpenCodeHighlighter>,
    // Transient state (dropped after parse)
    tag_stack: Vec<Tag<'a>>,
    table_state: Option<TableState>,
    depth: usize,
    /// Current blockquote nesting depth (0 = not in any blockquote).
    /// Used to determine which `>` on a line belongs to the current level.
    bq_depth: usize,
    last_checkpoint: Option<(CheckpointKind, usize)>,
    /// Maximum width for rendered tables (in display columns).
    /// When `Some(w)`, column widths are shrunk proportionally so the table
    /// fits within `w` columns.  When `None`, columns use natural widths.
    max_table_width: Option<usize>,
    /// Monotonically increasing counter for assigning stable link IDs.
    /// Persisted across `rerender_tail` calls via the streaming renderer.
    link_id_counter: u32,
    /// When `true` (default), CommonMark soft breaks inside a paragraph
    /// collapse to a single space. When `false`, the source newline is
    /// preserved so each source line surfaces as its own visual line —
    /// required by the line-numbered plan preview, where rendered lines
    /// must map 1:1 to file lines.
    collapse_soft_breaks: bool,
    /// In-progress fenced code block, set between its start and end events.
    pending_code_block: Option<PendingCodeBlock>,
}

/// Custom word separator for table cells.
///
/// Like `AsciiSpace`, but also treats punctuation and symbol characters as
/// break opportunities when followed by a letter.  This lets tables break
/// lines at e.g. `foo/bar` or `hello-world` without ever splitting mid-word.
///
/// For each break point, the punctuation character is attached to whichever
/// side produces the shorter maximum segment — e.g. `ABCD-EFG` becomes
/// `ABCD` + `-EFG` (max 4) rather than `ABCD-` + `EFG` (max 5).
///
/// Only `,` and `.` between digits suppress the break — these are number
/// formatting (e.g. `$145,000`, `3.14`).  All other punctuation can break
/// even between digits, so phone numbers (`555-0101`), dates (`2019-03-15`)
/// etc. become breakable.
///
/// Returns `true` for `<br>`, `<br/>`, `<br />`, etc. (case-insensitive).
fn is_br_tag(html: &str) -> bool {
    let Some(inner) = html
        .trim()
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
    else {
        return false;
    };
    let tag = inner.trim();
    let tag = tag.strip_suffix('/').map_or(tag, str::trim);
    tag.eq_ignore_ascii_case("br")
}

/// URLs (validated via `url::Url::parse`) are treated as unbreakable
/// words — no break points are placed within a URL so that terminal
/// hyperlink detection (Cmd+Click) continues to work when cells wrap.
pub(crate) fn cell_word_separator<'a>(
    line: &'a str,
) -> Box<dyn Iterator<Item = textwrap::core::Word<'a>> + 'a> {
    // Pass 1: find break-point byte positions.
    // A break point sits between a punctuation/symbol char and the alphabetic
    // char that follows it.  We record (break_byte_idx, punct_byte_start)
    // where break_byte_idx is where the next word would start if we attach
    // the punct char to the left, and punct_byte_start is where the punct
    // char begins (for attaching it to the right instead).
    let mut breaks: Vec<(usize, usize)> = Vec::new();
    {
        let mut in_whitespace = false;
        let mut after_break_char = false;
        let mut prev_is_digit = false; // was the *previous* char a digit?
        let mut digit_before_break = false; // was the char before the break char a digit?
        let mut last_break_ch: char = '\0';
        let mut break_char_start: usize = 0;
        for (idx, ch) in line.char_indices() {
            let is_space = ch == ' ';
            let is_break_char = !is_space && !ch.is_alphanumeric();

            // After a break char, decide if we should split here.
            //
            // Two cases allow a break:
            //  a) Followed by a letter → always break (new word boundary).
            //  b) Followed by a digit AND the char before the punct was
            //     also a digit → break, UNLESS the punct is `,` or `.`
            //     (number formatting like `$145,000` or `3.14`).
            //
            // This means:
            //  - `foo/bar` breaks (letter after punct)           ✓
            //  - `555-0101` breaks (digit-hyphen-digit)          ✓
            //  - `$145,000` stays (digit-comma-digit)            ✓
            //  - `$145` stays (no digit before `$`)              ✓
            //  - `EMP-1001` breaks at hyphen (letter before it)  ✓
            let should_break = if in_whitespace && !is_space {
                true
            } else if after_break_char {
                if ch.is_alphabetic() {
                    true
                } else if ch.is_ascii_digit() && digit_before_break {
                    // digit-punct-digit: only break for non-formatting punct
                    last_break_ch != ',' && last_break_ch != '.'
                } else {
                    false
                }
            } else {
                false
            };

            if should_break {
                if in_whitespace {
                    breaks.push((idx, idx));
                } else {
                    breaks.push((idx, break_char_start));
                }
            }

            if is_break_char {
                break_char_start = idx;
                last_break_ch = ch;
                digit_before_break = prev_is_digit;
            }
            prev_is_digit = ch.is_ascii_digit();
            in_whitespace = is_space;
            after_break_char = is_break_char;
        }
    }

    // Filter out break points that fall inside a URL.
    // Each whitespace-delimited token is tested with `url::Url::parse`;
    // tokens that parse as valid URLs are protected from splitting.
    let url_ranges: Vec<Range<usize>> = {
        let mut ranges = Vec::new();
        let mut pos = 0;
        for token in line.split_whitespace() {
            let start = line[pos..].find(token).unwrap() + pos;
            let end = start + token.len();
            if url::Url::parse(token).is_ok() {
                ranges.push(start..end);
            }
            pos = end;
        }
        ranges
    };
    breaks.retain(|&(break_pos, _)| {
        !url_ranges
            .iter()
            .any(|r| break_pos > r.start && break_pos < r.end)
    });

    // Pass 2: decide attachment for each break point.
    // For punct breaks, choose the side that minimizes max(left_len, right_len).
    let mut split_positions: Vec<usize> = Vec::with_capacity(breaks.len());
    {
        let len = line.len();
        for (i, &(attach_left, attach_right)) in breaks.iter().enumerate() {
            if attach_left == attach_right {
                // Whitespace break — no choice.
                split_positions.push(attach_left);
            } else {
                // Determine segment boundaries for this break.
                let seg_start = if i == 0 { 0 } else { split_positions[i - 1] };
                let seg_end = if i + 1 < breaks.len() {
                    // Use the leftward attachment of the next break as a
                    // conservative estimate of the right segment end.
                    breaks[i + 1].0
                } else {
                    len
                };

                let left_if_attach_left = unicode_display_width(&line[seg_start..attach_left]);
                let right_if_attach_left = unicode_display_width(&line[attach_left..seg_end]);
                let max_attach_left = left_if_attach_left.max(right_if_attach_left);

                let left_if_attach_right = unicode_display_width(&line[seg_start..attach_right]);
                let right_if_attach_right = unicode_display_width(&line[attach_right..seg_end]);
                let max_attach_right = left_if_attach_right.max(right_if_attach_right);

                if max_attach_right < max_attach_left {
                    split_positions.push(attach_right);
                } else {
                    split_positions.push(attach_left);
                }
            }
        }
    }

    // Pass 3: emit Words at the chosen split positions.
    let mut pos = 0usize;
    let mut idx = 0usize;
    Box::new(std::iter::from_fn(move || {
        if pos >= line.len() {
            return None;
        }
        let end = if idx < split_positions.len() {
            let e = split_positions[idx];
            idx += 1;
            e
        } else {
            line.len()
        };
        let word = textwrap::core::Word::from(&line[pos..end]);
        pos = end;
        Some(word)
    }))
}

/// Output of [`MarkdownParser::format_table`]: the rendered lines of a single table.
#[derive(Default)]
struct FormattedTable {
    /// Plain-text lines (for ANSI rendering).
    lines: Vec<String>,
    /// Styled lines (for ratatui rendering).
    styled_lines: Vec<Line<'static>>,
    /// Per-line source offset within the table (0 = header, 1 = separator, 2+ = body rows).
    line_source_offsets: Vec<usize>,
    /// Hyperlinks (in table-local line coordinates) for links inside cells.
    hyperlinks: Vec<TableHyperlink>,
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/parse/events.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/parse/tags.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/parse/inline.rs"));

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/parse/tables.rs"));

/// Parsed markdown ready for rendering.
///
/// Created by `MarkdownParser::parse()`. Contains the source text, style,
/// and a reference to the populated buffers. Transient parsing state has
/// been dropped at this point.
pub struct ParsedMarkdown<'a, 'b> {
    pub(crate) text: &'a str,
    pub(crate) ms: MarkdownStyle,
    pub(crate) buffers: &'b mut MarkdownBuffers,
    pub(crate) last_checkpoint: Option<(CheckpointKind, usize)>,
    pub(crate) next_link_id: u32,
}

impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    pub fn new(
        text: &'a str,
        ms: MarkdownStyle,
        buffers: &'b mut MarkdownBuffers,
        last_checkpoint: Option<(CheckpointKind, usize)>,
        next_link_id: u32,
    ) -> Self {
        Self {
            text,
            ms,
            buffers,
            last_checkpoint,
            next_link_id,
        }
    }
}
