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
