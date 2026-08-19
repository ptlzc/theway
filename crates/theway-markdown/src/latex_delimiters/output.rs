/// Emit `interior` as a canonical `$$…$$` span, joining interior lines.
///
/// Single-line interiors are emitted verbatim (bare `$$…$$` input passes
/// through byte-for-byte, keeping the pass idempotent). Multi-line interiors
/// have each line trimmed and joined with a single space so CommonMark block
/// parsing (setext underlines, list items, headings) cannot split the span;
/// TeX treats the newlines as spaces, so rendering is unchanged.
fn emit_display_span(out: &mut String, interior: &str) {
    out.push_str("$$");
    push_joined_lines(out, interior);
    out.push_str("$$");
}

/// Push `text` onto `out`; if it spans multiple lines, trim each line and
/// join the non-empty ones with single spaces (single-line text is verbatim).
fn push_joined_lines(out: &mut String, text: &str) {
    if !text.contains('\n') {
        out.push_str(text);
        return;
    }
    let mut first = true;
    for line in text.lines() {
        let trimmed = line.trim_matches([' ', '\t', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(trimmed);
        first = false;
    }
}

enum EnvScan {
    Convert(usize),
    No,
    NeedMore,
}

/// Match `\begin{equation}` / `\end{equation}` (and starred variants) at `i`.
fn match_env(buf: &str, i: usize, final_flush: bool) -> EnvScan {
    let rest = &buf[i..];
    let mut best: Option<usize> = None;
    let mut could_extend = false;
    for tok in ENV_TOKENS {
        if rest.len() >= tok.len() {
            if rest.starts_with(tok) {
                best = Some(best.map_or(tok.len(), |b: usize| b.max(tok.len())));
            }
        } else if tok.starts_with(rest) {
            could_extend = true;
        }
    }
    if let Some(len) = best {
        // `\begin{equation}` is not a prefix of `\begin{equation*}` (char 16 is
        // `}` vs `*`), so the longest full match is unambiguous.
        return EnvScan::Convert(len);
    }
    if could_extend && !final_flush {
        return EnvScan::NeedMore;
    }
    EnvScan::No
}
