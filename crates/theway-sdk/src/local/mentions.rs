//! `@file` mention injection.
//!
//! When the user types a prompt containing `@<path>` tokens, the REPL resolves each path
//! against the current working directory, reads the file, and prepends a small attachment
//! block to the prompt. The agent never sees the raw `@path` token — it sees:
//!
//! ```text
//! Files in context:
//! <file path="src/foo.rs">
//! …content…
//! </file>
//!
//! <user's original text>
//! ```
//!
//! Size cap: 64 KiB per file. Files larger than that are truncated with a "(truncated at N
//! KiB)" marker. The original `@path` token stays in the user's text so the LLM sees what
//! the user actually typed.

use std::path::{Path, PathBuf};

const MAX_BYTES: usize = 64 * 1024;

/// Returns `(rewritten_prompt, resolved_paths)`. If `input` has no `@<path>` tokens, the
/// rewritten prompt is the original and `resolved_paths` is empty.
pub async fn expand(input: &str, cwd: &Path) -> (String, Vec<PathBuf>) {
    let mentions = extract_mentions(input);
    if mentions.is_empty() {
        return (input.to_string(), Vec::new());
    }
    let mut blocks = Vec::new();
    let mut resolved = Vec::new();
    for rel in &mentions {
        let path = cwd.join(rel);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let (body, truncated) = truncate(&text);
                let display = rel.to_string();
                let block = if truncated {
                    format!(
                        "<file path=\"{display}\">\n{body}\n\n(truncated at {} KiB)\n</file>",
                        MAX_BYTES / 1024
                    )
                } else {
                    format!("<file path=\"{display}\">\n{body}\n</file>")
                };
                blocks.push(block);
                resolved.push(path);
            }
            Err(e) => {
                blocks.push(format!(
                    "<file path=\"{rel}\" error=\"{e}\" />",
                    rel = rel,
                    e = e
                ));
            }
        }
    }
    let header = format!("Files in context:\n{}\n\n", blocks.join("\n"));
    (format!("{header}{input}"), resolved)
}

/// Scan for `@<path>` tokens. Stops a path at whitespace, semicolon, comma, parenthesis, or
/// quote. Leading punctuation around the `@` (e.g. wrapping in parens) is fine.
fn extract_mentions(input: &str) -> Vec<String> {
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

fn truncate(text: &str) -> (String, bool) {
    if text.len() <= MAX_BYTES {
        return (text.to_string(), false);
    }
    // Trim at a char boundary <= MAX_BYTES.
    let mut end = MAX_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extracts_simple_mention() {
        assert_eq!(
            extract_mentions("look at @src/foo.rs please"),
            vec!["src/foo.rs".to_string()]
        );
    }

    #[test]
    fn extracts_multiple_with_punctuation() {
        let m = extract_mentions("review @a.rs, @b/c.rs and (@d.rs)");
        assert_eq!(
            m,
            vec!["a.rs".to_string(), "b/c.rs".to_string(), "d.rs".to_string()]
        );
    }

    #[test]
    fn ignores_at_inside_email() {
        assert!(extract_mentions("ping user@host.com").is_empty());
    }

    #[tokio::test]
    async fn expand_reads_files_and_falls_back_to_error_block_on_missing() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("hello.txt");
        std::fs::write(&p, "hi there").unwrap();
        let (out, resolved) = expand("look at @hello.txt and @missing.txt", dir.path()).await;
        assert!(out.starts_with("Files in context:"));
        assert!(out.contains("<file path=\"hello.txt\">"), "{out}");
        assert!(out.contains("hi there"), "{out}");
        assert!(out.contains("<file path=\"missing.txt\""), "{out}");
        assert!(out.contains("look at @hello.txt"), "original kept: {out}");
        assert_eq!(
            resolved.len(),
            1,
            "only existing files in resolved: {resolved:?}"
        );
    }

    #[tokio::test]
    async fn expand_returns_input_unchanged_when_no_mentions() {
        let dir = TempDir::new().unwrap();
        let (out, resolved) = expand("just a regular prompt", dir.path()).await;
        assert_eq!(out, "just a regular prompt");
        assert!(resolved.is_empty());
    }
}
