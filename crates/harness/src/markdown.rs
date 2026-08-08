//! Streaming-friendly Markdown → ANSI renderer. The TUI calls `render(line)` per assistant
//! text line; the renderer maps inline `**bold**`, `*italic*`, `` `code` ``, headings, and
//! code-fence blocks to terminal escape sequences. Plain text passes through unchanged.
//!
//! This is intentionally simple: no full CommonMark parse, no nested emphasis precedence, no
//! reference links. It's the streaming-time pass that the full TUI overhaul (c4pt0r/theway#2)
//! will eventually replace with an incremental syntect-backed renderer.

#![allow(dead_code)]

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const ITALIC: &str = "\x1b[3m";
const DIM: &str = "\x1b[2m";
const CODE: &str = "\x1b[2;36m"; // dim cyan
const HEADING: &str = "\x1b[1;34m"; // bold blue

/// Render one logical line of Markdown. Stateless on input. The caller decides whether to
/// track code-fence state across lines (use [`Renderer`] for that).
pub fn render_line(line: &str) -> String {
    if let Some(level) = heading_level(line) {
        let body = &line[level + 1..];
        return format!("{HEADING}{}{}{RESET}", "#".repeat(level), body);
    }
    render_inline(line)
}

fn heading_level(line: &str) -> Option<usize> {
    let mut level = 0;
    for c in line.chars() {
        if c == '#' {
            level += 1;
        } else if c == ' ' && level > 0 && level <= 6 {
            return Some(level);
        } else {
            return None;
        }
    }
    None
}

fn render_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // ` `code` `
        if bytes[i] == b'`' {
            if let Some(end) = find_byte(bytes, i + 1, b'`') {
                out.push_str(CODE);
                out.push_str(&line[i + 1..end]);
                out.push_str(RESET);
                i = end + 1;
                continue;
            }
        }
        // **bold**
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end) = find_double_star(bytes, i + 2) {
                out.push_str(BOLD);
                out.push_str(&line[i + 2..end]);
                out.push_str(RESET);
                i = end + 2;
                continue;
            }
        }
        // *italic* (avoid matching when it's the start of a list bullet "* ").
        if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] != b' ' && bytes[i + 1] != b'*' {
            if let Some(end) = find_single_star(bytes, i + 1) {
                out.push_str(ITALIC);
                out.push_str(&line[i + 1..end]);
                out.push_str(RESET);
                i = end + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn find_byte(buf: &[u8], from: usize, b: u8) -> Option<usize> {
    buf[from..].iter().position(|&x| x == b).map(|i| from + i)
}

fn find_double_star(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'*' && buf[i + 1] == b'*' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_single_star(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < buf.len() {
        if buf[i] == b'*' {
            // Reject `**` (handled by bold pass).
            if i + 1 < buf.len() && buf[i + 1] == b'*' {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Stateful renderer that tracks code-fence boundaries across lines. Inside a fenced block,
/// content is dim-cyan; the fence delimiters themselves are dimmed.
pub struct Renderer {
    in_fence: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self { in_fence: false }
    }
    pub fn render_line(&mut self, line: &str) -> String {
        if line.starts_with("```") {
            self.in_fence = !self.in_fence;
            return format!("{DIM}{line}{RESET}");
        }
        if self.in_fence {
            return format!("{CODE}{line}{RESET}");
        }
        render_line(line)
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_inline_emits_ansi() {
        let s = render_line("hello **world**!");
        assert!(s.contains("\x1b[1m"));
        assert!(s.contains("world"));
        assert!(s.contains("\x1b[0m"));
        // No raw `**` left.
        assert!(!s.contains("**"));
    }

    #[test]
    fn italic_inline_emits_ansi() {
        let s = render_line("an *italic* word");
        assert!(s.contains("\x1b[3m"));
        assert!(s.contains("italic"));
    }

    #[test]
    fn code_inline_emits_ansi() {
        let s = render_line("call `foo()`");
        assert!(s.contains(CODE));
        assert!(s.contains("foo()"));
        assert!(!s.contains('`'));
    }

    #[test]
    fn heading_marked() {
        let s = render_line("## Section");
        assert!(s.contains(HEADING));
        assert!(s.contains("Section"));
    }

    #[test]
    fn renderer_tracks_fence_across_lines() {
        let mut r = Renderer::new();
        let open = r.render_line("```rust");
        assert!(open.contains(DIM));
        let body = r.render_line("let x = 1;");
        assert!(body.contains(CODE));
        let close = r.render_line("```");
        assert!(close.contains(DIM));
        let after = r.render_line("plain text");
        assert!(!after.contains(CODE), "should exit fence: {after}");
    }

    #[test]
    fn unclosed_backtick_is_left_alone() {
        let s = render_line("partial `code");
        assert_eq!(s, "partial `code");
    }
}
