#[cfg(test)]
mod token_soup_stress {
    use crate::{LatexDelimiterNormalizer, normalize_latex_delimiters};

    /// Randomized delimiter-soup stress. Two invariants are universal and
    /// pinned here for arbitrary input:
    ///
    /// 1. the normalizer never panics;
    /// 2. streaming char-by-char matches the one-shot output (chunk-split
    ///    invariance — what production streaming actually relies on).
    ///
    /// Full byte-idempotency is deliberately *not* asserted on soup: a
    /// conversion can glue a new `$$` out of adjacent tokens (e.g. `$` + an
    /// unmatched `\)` → `$$`), which a second pass would then scan as a
    /// display opener. Production normalizes exactly once per stream (the
    /// streaming renderer's `clone()` re-appends already-normalized source
    /// verbatim), and idempotency for realistic documents is pinned by the
    /// `idempotent` test's curated list.
    #[test]
    fn token_soup_never_panics_and_streams_consistently() {
        const TOKENS: [&str; 18] = [
            "$$",
            "$",
            "\\[",
            "\\]",
            "\\(",
            "\\)",
            "\n",
            "\n\n",
            "=",
            "-",
            ">",
            "`",
            "```",
            "x y",
            "\\begin{equation}",
            "\\end{equation}",
            "\\\\",
            "\r\n",
        ];
        // Simple deterministic LCG so failures are reproducible.
        let mut state: u64 = 0x243F6A8885A308D3;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        for _ in 0..4000 {
            let len = 1 + next() % 12;
            let doc: String = (0..len).map(|_| TOKENS[next() % TOKENS.len()]).collect();
            let oneshot = normalize_latex_delimiters(&doc);
            // Char-by-char streaming must match one-shot.
            let mut nz = LatexDelimiterNormalizer::new();
            let mut got = String::new();
            for ch in doc.chars() {
                got.push_str(&nz.push(ch.encode_utf8(&mut [0u8; 4])));
            }
            got.push_str(&nz.finish());
            assert_eq!(got, oneshot, "stream mismatch for {doc:?}");
        }
    }
}
