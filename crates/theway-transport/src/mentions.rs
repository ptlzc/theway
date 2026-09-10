//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! `@file` mention injection — the single mention parser for the daemon prompt
//! intake, plus the legacy prompt-expansion helper.
//!
//! When the user types a prompt containing `@<path>` tokens, [`mentions`] lists them; the
//! daemon resolves each path once at admission, reads the file, and records it as a
//! structured input part. [`expand`] keeps the legacy behaviour for the text path: it
//! resolves each path against the current working directory, reads the file, and appends a
//! small attachment block to the END of the user message. Missing/unrecognized `@` paths are
//! silently skipped — they add no error block and no prompt. The agent then sees:
//!
//! ```text
//! <user's original text>
//!
//! Files in context:
//! <file path="src/foo.rs">
//! …content…
//! </file>
//! ```
//!
//! Size cap: [`MAX_MENTION_BYTES`] per file, shared by both paths. Files larger than that are
//! truncated with a "(truncated at N KiB)" marker. The original `@path` token stays in the
//! user's text so the LLM sees what the user actually typed.

use std::path::{Path, PathBuf};

/// Per-file cap for `@path` mentions: larger files are truncated with a `(truncated at N KiB)` marker.
pub const MAX_MENTION_BYTES: usize = 64 * 1024;

/// The `@<path>` tokens in `input`, in order, as the user typed them. A path stops at
/// whitespace, semicolon, comma, parenthesis, or quote; leading punctuation around the `@`
/// (e.g. wrapping in parens) is fine.
///
/// The daemon resolves these once at admission, when it builds the turn's structured input
/// record; [`expand`] resolves the same tokens for the legacy text path.
pub fn mentions(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '@' {
            i += 1;
            continue;
        }
        // `@` must be at a word boundary — not in the middle of an email address etc.
        if i > 0 {
            let prev = chars[i - 1];
            if prev.is_alphanumeric() || prev == '_' || prev == '.' {
                i += 1;
                continue;
            }
        }
        let mut j = i + 1;
        while j < chars.len() {
            let c = chars[j];
            if c.is_whitespace() || matches!(c, ';' | ',' | '(' | ')' | '"' | '\'' | '`') {
                break;
            }
            j += 1;
        }
        if j > i + 1 {
            let path: String = chars[i + 1..j].iter().collect();
            // Strip trailing punctuation that's likely sentence punctuation.
            let path = path.trim_end_matches(['.', '!', '?', ':']);
            if !path.is_empty() {
                out.push(path.to_string());
            }
        }
        i = j;
    }
    out
}

/// Truncate `text` at [`MAX_MENTION_BYTES`] on a char boundary, reporting whether it truncated.
pub fn truncate_mention(text: &str) -> (String, bool) {
    if text.len() <= MAX_MENTION_BYTES {
        return (text.to_string(), false);
    }
    // Trim at a char boundary <= MAX_MENTION_BYTES.
    let mut end = MAX_MENTION_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Returns `(rewritten_prompt, resolved_paths)`. If `input` has no `@<path>` tokens,
/// or every mention is an unrecognized/missing path, the rewritten prompt is the
/// original and `resolved_paths` is empty. Recognized files are appended after the
/// user's original text; failed resolutions are silently skipped.
pub async fn expand(input: &str, cwd: &Path) -> (String, Vec<PathBuf>) {
    let mentions = mentions(input);
    if mentions.is_empty() {
        return (input.to_string(), Vec::new());
    }
    let mut blocks = Vec::new();
    let mut resolved = Vec::new();
    for rel in &mentions {
        let path = cwd.join(rel);
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            let (body, truncated) = truncate_mention(&text);
            let display = rel.to_string();
            let block = if truncated {
                format!(
                    "<file path=\"{display}\">\n{body}\n\n(truncated at {} KiB)\n</file>",
                    MAX_MENTION_BYTES / 1024
                )
            } else {
                format!("<file path=\"{display}\">\n{body}\n</file>")
            };
            blocks.push(block);
            resolved.push(path);
        }
    }
    if blocks.is_empty() {
        return (input.to_string(), Vec::new());
    }
    let attachments = format!("Files in context:\n{}", blocks.join("\n"));
    (format!("{input}\n\n{attachments}"), resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extracts_simple_mention() {
        assert_eq!(
            mentions("look at @src/foo.rs please"),
            vec!["src/foo.rs".to_string()]
        );
    }

    #[test]
    fn extracts_multiple_with_punctuation() {
        let m = mentions("review @a.rs, @b/c.rs and (@d.rs)");
        assert_eq!(
            m,
            vec!["a.rs".to_string(), "b/c.rs".to_string(), "d.rs".to_string()]
        );
    }

    #[test]
    fn ignores_at_inside_email() {
        assert!(mentions("ping user@host.com").is_empty());
    }

    #[test]
    fn truncate_mention_reports_the_cap() {
        let short = "hi";
        assert_eq!(truncate_mention(short), (short.to_string(), false));

        let exact = "x".repeat(MAX_MENTION_BYTES);
        assert_eq!(truncate_mention(&exact), (exact.clone(), false));

        let long = "y".repeat(MAX_MENTION_BYTES + 1);
        let (body, truncated) = truncate_mention(&long);
        assert!(truncated);
        assert_eq!(body.len(), MAX_MENTION_BYTES);
    }

    #[test]
    fn truncate_mention_stays_on_a_char_boundary() {
        // A 2-byte char straddling the cap is dropped whole.
        let mut text = "a".repeat(MAX_MENTION_BYTES - 1);
        text.push('é');
        let (body, truncated) = truncate_mention(&text);
        assert!(truncated);
        assert_eq!(body.len(), MAX_MENTION_BYTES - 1);
        assert!(body.is_char_boundary(body.len()));
    }

    #[tokio::test]
    async fn expand_reads_files_and_appends_them_after_the_user_text() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("hello.txt");
        std::fs::write(&p, "hi there").unwrap();
        let (out, resolved) = expand("look at @hello.txt and @missing.txt", dir.path()).await;
        assert!(
            out.starts_with("look at @hello.txt"),
            "original text stays first: {out}"
        );
        let header = out.find("Files in context:").expect("header present");
        assert!(header > 0, "attachments are appended, not prepended: {out}");
        assert!(out.contains("<file path=\"hello.txt\">"), "{out}");
        assert!(out.contains("hi there"), "{out}");
        assert!(
            !out.contains("error="),
            "unrecognized mentions are skipped without an error block: {out}"
        );
        assert_eq!(
            resolved.len(),
            1,
            "only existing files in resolved: {resolved:?}"
        );
    }

    #[tokio::test]
    async fn expand_skips_mentions_that_do_not_resolve() {
        let dir = TempDir::new().unwrap();
        let (out, resolved) = expand("check @missing.txt please", dir.path()).await;
        assert_eq!(out, "check @missing.txt please");
        assert!(resolved.is_empty());
    }

    #[tokio::test]
    async fn expand_returns_input_unchanged_when_no_mentions() {
        let dir = TempDir::new().unwrap();
        let (out, resolved) = expand("just a regular prompt", dir.path()).await;
        assert_eq!(out, "just a regular prompt");
        assert!(resolved.is_empty());
    }
}
