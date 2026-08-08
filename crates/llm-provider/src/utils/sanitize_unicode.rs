//! UTF-16 surrogate sanitization. 1:1 port of `packages/ai/src/utils/sanitize-unicode.ts`.
//!
//! Rust `&str` is always valid UTF-8, so unpaired surrogates cannot actually be present —
//! they'd have failed `str::from_utf8` upstream. This function is therefore mostly a no-op for
//! Rust input. We keep it for symmetry with the TS API and for the rare case of decoding
//! provider chunks via `String::from_utf16_lossy`, where lone surrogates could survive.

/// Sanitize unpaired surrogates. For valid Rust strings this is the identity.
pub fn sanitize_surrogates(text: &str) -> String {
    text.to_owned()
}

/// Variant that operates on `&[u16]` (decoded UTF-16 buffer). Drops any unpaired surrogate.
pub fn sanitize_surrogates_u16(buf: &[u16]) -> String {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        let c = buf[i];
        if (0xD800..=0xDBFF).contains(&c) {
            if i + 1 < buf.len() && (0xDC00..=0xDFFF).contains(&buf[i + 1]) {
                out.push(c);
                out.push(buf[i + 1]);
                i += 2;
                continue;
            }
            // unpaired high — drop
            i += 1;
            continue;
        }
        if (0xDC00..=0xDFFF).contains(&c) {
            // unpaired low — drop
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf16_lossy(&out)
}
