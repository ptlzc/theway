//! `grep` tool — line-based regex match across a directory tree. Models
//! `packages/coding-agent/src/core/tools/grep.ts` at a simplified level: no thread pool, no
//! ripgrep delegation, just `ignore::WalkBuilder` + the `regex` crate.
//!
//! `output_mode` mirrors the enhanced-tools `grep.ts`:
//!   - `content` (default): ripgrep-style grouped output — file path printed once, merged
//!     overlapping contexts, `--` between disjoint regions, `:` for match lines, `-` for
//!     context lines.
//!   - `files_with_matches`: just the file paths (deduped).
//!   - `count`: `count<TAB>path` per file (ripgrep `--count` style).
//!
//! Result count is capped (`max_results`); long lines are previewed around the match with
//! `[line truncated]` markers.

use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RESULTS: usize = 100;
const DEFAULT_MAX_FILES: usize = 5_000;
const MAX_MATCH_LINE_CHARS: usize = 500;

pub struct GrepTool;

#[async_trait]
impl AgentTool for GrepTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "grep"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let pattern = params
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `pattern`"))?;
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let glob = params.get("glob").and_then(|v| v.as_str());
        let case_insensitive = params
            .get("case_insensitive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let output_mode = params.get("output_mode").and_then(|v| v.as_str());
        match output_mode {
            None | Some("content") | Some("files_with_matches") | Some("count") => {}
            Some(other) => {
                return Err(AgentToolError::from(format!(
                    "invalid output_mode {other:?}; expected 'content', 'files_with_matches', or 'count'"
                )));
            }
        }
        let context_lines = params
            .get("context_lines")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);
        let max_matches = params
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);
        let mut builder = regex::RegexBuilder::new(pattern);
        builder.case_insensitive(case_insensitive);
        let re: Regex = builder
            .build()
            .map_err(|e| AgentToolError::from(format!("regex: {e}")))?;

        // Walk synchronously inside spawn_blocking so .gitignore + sibling files are honored
        // by `ignore` and we don't block the runtime.
        let path_owned = path.to_string();
        let glob = glob.map(str::to_string);
        let re_clone = re.clone();
        let cancel_clone = cancel.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<Vec<RawMatch>, String> {
            let mut walker = WalkBuilder::new(&path_owned);
            walker.standard_filters(true).hidden(true);
            if let Some(g) = &glob {
                let mut tb = ignore::types::TypesBuilder::new();
                tb.add("g", g).map_err(|e| e.to_string())?;
                tb.select("g");
                let types = tb.build().map_err(|e| e.to_string())?;
                walker.types(types);
            }
            let walker = walker.build();
            let mut out: Vec<RawMatch> = Vec::new();
            let mut files_scanned = 0usize;
            for entry in walker {
                if cancel_clone.is_cancelled() {
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    continue;
                }
                files_scanned += 1;
                if files_scanned > DEFAULT_MAX_FILES {
                    break;
                }
                let p = entry.path();
                let body = match std::fs::read_to_string(p) {
                    Ok(b) => b,
                    Err(_) => continue, // binary or unreadable; skip
                };
                let lines: Vec<&str> = body.lines().collect();
                for (i, line) in lines.iter().enumerate() {
                    if re_clone.is_match(line) {
                        let before_start = i.saturating_sub(context_lines);
                        let after_end = (i + 1 + context_lines).min(lines.len());
                        out.push(RawMatch {
                            path: p.display().to_string(),
                            lineno: i + 1,
                            line: (*line).to_string(),
                            context_before: lines[before_start..i]
                                .iter()
                                .map(|l| truncate_line(l))
                                .collect(),
                            context_after: lines[i + 1..after_end]
                                .iter()
                                .map(|l| truncate_line(l))
                                .collect(),
                        });
                        if out.len() >= max_matches {
                            return Ok(out);
                        }
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| AgentToolError::from(format!("spawn_blocking: {e}")))?;
        let matches = result.map_err(AgentToolError::from)?;

        let output_mode = output_mode.unwrap_or("content");
        let (text, truncated_lines) = if matches.is_empty() {
            (format!("No matches for /{pattern}/ in {path}"), 0)
        } else {
            match output_mode {
                "files_with_matches" => (format_files_with_matches(&matches), 0),
                "count" => (format_counts(&matches), 0),
                _ => format_content(&matches, &re, max_matches),
            }
        };

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({
                "matches": matches.len(),
                "truncated_lines": truncated_lines,
                "max_match_line_chars": MAX_MATCH_LINE_CHARS,
                "outputMode": output_mode,
            }),
            terminate: None,
        })
    }
}

struct RawMatch {
    path: String,
    lineno: usize,
    line: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

/// `files_with_matches` mode: deduped file paths, one per line, in scan order.
fn format_files_with_matches(matches: &[RawMatch]) -> String {
    let mut files: Vec<&str> = Vec::new();
    for m in matches {
        if !files.contains(&m.path.as_str()) {
            files.push(m.path.as_str());
        }
    }
    files.join("\n")
}

/// `count` mode: `count<TAB>path` per file, ripgrep `--count` style.
fn format_counts(matches: &[RawMatch]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for m in matches {
        match counts.iter_mut().find(|(p, _)| *p == m.path.as_str()) {
            Some((_, c)) => *c += 1,
            None => counts.push((m.path.as_str(), 1)),
        }
    }
    counts
        .iter()
        .map(|(p, c)| format!("{c}\t{p}"))
        .collect::<Vec<_>>()
        .join("\n")
}
/// `content` mode: port of enhanced-tools `grep.ts` `formatContentMatches` — file path
/// printed once as a header, per-file line map dedups overlapping contexts, contiguous
/// lines form one region, disjoint regions are separated by `--`. Match lines use `:`,
/// context lines `-`; line numbers are right-aligned to the widest line number in the file.
/// Returns the text plus how many match lines were preview-truncated.
fn format_content(matches: &[RawMatch], re: &Regex, max_matches: usize) -> (String, usize) {
    let mut by_file: BTreeMap<String, BTreeMap<usize, (String, bool)>> = BTreeMap::new();
    let mut truncated_lines = 0usize;
    for m in matches {
        let file_map = by_file.entry(m.path.clone()).or_default();
        let before_len = m.context_before.len();
        for (i, l) in m.context_before.iter().enumerate() {
            let ln = m.lineno - before_len + i;
            file_map.entry(ln).or_insert((l.clone(), false));
        }
        let (preview, was_truncated) = preview_match_line(&m.line, re.find(&m.line));
        file_map.insert(m.lineno, (preview, true));
        if was_truncated {
            truncated_lines += 1;
        }
        for (i, l) in m.context_after.iter().enumerate() {
            let ln = m.lineno + 1 + i;
            file_map.entry(ln).or_insert((l.clone(), false));
        }
    }

    let mut out: Vec<String> = Vec::new();
    for (file_path, line_map) in &by_file {
        let line_nums: Vec<usize> = line_map.keys().copied().collect();
        let width = line_nums.last().map(|n| n.to_string().len()).unwrap_or(1);
        // Split into contiguous regions (adjacent line numbers differ by 1).
        let mut regions: Vec<Vec<usize>> = Vec::new();
        let mut cur: Vec<usize> = vec![line_nums[0]];
        for i in 1..line_nums.len() {
            if line_nums[i] == line_nums[i - 1] + 1 {
                cur.push(line_nums[i]);
            } else {
                regions.push(std::mem::take(&mut cur));
                cur = vec![line_nums[i]];
            }
        }
        regions.push(cur);

        out.push(file_path.clone());
        for (idx, region) in regions.iter().enumerate() {
            if idx > 0 {
                out.push("--".to_string());
            }
            for ln in region {
                let (content, is_match) = &line_map[ln];
                let sep = if *is_match { ":" } else { "-" };
                out.push(format!("{ln:>width$}{sep} {content}", width = width));
            }
        }
        out.push(String::new());
    }

    let mut text = String::new();
    if matches.len() >= max_matches {
        text.push_str(&format!("... ({max_matches} matches shown, may be more)\n"));
    }
    text.push_str(out.join("\n").trim_end());
    if truncated_lines > 0 {
        text.push_str(&format!(
            "\n[{truncated_lines} long matching line(s) truncated to {MAX_MATCH_LINE_CHARS} chars]\n"
        ));
    }
    (text, truncated_lines)
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_MATCH_LINE_CHARS {
        line.to_string()
    } else {
        let preview: String = line.chars().take(MAX_MATCH_LINE_CHARS).collect();
        format!("{preview}...[line truncated]")
    }
}

fn preview_match_line(line: &str, match_range: Option<regex::Match<'_>>) -> (String, bool) {
    if line.chars().count() <= MAX_MATCH_LINE_CHARS {
        return (line.to_string(), false);
    }

    let Some(match_range) = match_range else {
        let preview: String = line.chars().take(MAX_MATCH_LINE_CHARS).collect();
        return (format!("{preview}...[line truncated]"), true);
    };

    let match_start = line[..match_range.start()].chars().count();
    let match_len = line[match_range.start()..match_range.end()]
        .chars()
        .count()
        .max(1);
    let visible_match_len = match_len.min(MAX_MATCH_LINE_CHARS);
    let context_budget = MAX_MATCH_LINE_CHARS.saturating_sub(visible_match_len);
    let before_budget = context_budget / 2;
    let after_budget = context_budget - before_budget;
    let start_char = match_start.saturating_sub(before_budget);
    let end_char = match_start + visible_match_len + after_budget;
    let total_chars = line.chars().count();

    let mut preview = String::new();
    if start_char > 0 {
        preview.push_str("[line truncated]...");
    }
    preview.extend(
        line.chars()
            .skip(start_char)
            .take(end_char.saturating_sub(start_char).min(total_chars)),
    );
    if end_char < total_chars {
        preview.push_str("...[line truncated]");
    }
    (preview, true)
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "grep".into(),
    description: format!(
        "Search file contents using regular expressions. Honors .gitignore. \
         output_mode: 'files_with_matches' to just list matching files (cheapest, use first to locate); \
         'count' for per-file match counts; 'content' (default) for matched lines with optional context. \
         content output groups by file and merges overlapping contexts. \
         Output limited to {DEFAULT_MAX_RESULTS} matches."
    ),
    parameters: json!({
        "type": "object",
        "properties": {
            "pattern": { "type": "string", "description": "Regex pattern" },
            "path": { "type": "string", "description": "Directory to search (default: current)" },
            "glob": { "type": "string", "description": "Optional filename glob (e.g. *.rs)" },
            "output_mode": { "type": "string", "description": "'content' (default), 'files_with_matches', or 'count'" },
            "case_insensitive": { "type": "boolean", "description": "Case-insensitive match" },
            "context_lines": { "type": "number", "description": "Lines of context before/after each match (content mode)" },
            "max_results": { "type": "number", "description": "Max matches to return (default: 100)" },
        },
        "required": ["pattern"],
    }),
});
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn run(tool: &GrepTool, params: Value) -> String {
        let r = tool
            .execute("g", params, CancellationToken::new(), None)
            .await
            .unwrap();
        match &r.content[0] {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn finds_matches_in_file_tree() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world\nfoo bar\n").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), "another hello\n").unwrap();

        let tool = GrepTool;
        let r = tool
            .execute(
                "g",
                json!({ "pattern": "hello", "path": dir.path().to_str().unwrap() }),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        let text = match &r.content[0] {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("hello world"));
        assert!(text.contains("another hello"));
    }

    #[tokio::test]
    async fn truncates_very_long_matching_lines() {
        let dir = tempdir().unwrap();
        let long_line = format!("needle {}", "x".repeat(MAX_MATCH_LINE_CHARS + 100));
        std::fs::write(dir.path().join("a.txt"), long_line).unwrap();

        let tool = GrepTool;
        let r = tool
            .execute(
                "g",
                json!({ "pattern": "needle", "path": dir.path().to_str().unwrap() }),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        let text = match &r.content[0] {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("...[line truncated]"));
        assert!(text.contains("1 long matching line(s) truncated"));
        assert_eq!(r.details["truncated_lines"], 1);
        assert_eq!(r.details["max_match_line_chars"], MAX_MATCH_LINE_CHARS);
        assert!(!text.contains(&"x".repeat(MAX_MATCH_LINE_CHARS + 100)));
    }

    #[tokio::test]
    async fn long_line_preview_keeps_late_match_visible() {
        let dir = tempdir().unwrap();
        let long_line = format!("{} NEEDLE {}", "prefix".repeat(120), "suffix".repeat(120));
        std::fs::write(dir.path().join("a.txt"), long_line).unwrap();

        let tool = GrepTool;
        let r = tool
            .execute(
                "g",
                json!({ "pattern": "NEEDLE", "path": dir.path().to_str().unwrap() }),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        let text = match &r.content[0] {
            theway_llm_provider::UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("NEEDLE"));
        assert!(text.contains("[line truncated]..."));
        assert!(text.contains("...[line truncated]"));
        assert_eq!(r.details["truncated_lines"], 1);
    }
    #[tokio::test]
    async fn content_mode_merges_overlapping_contexts() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("a.txt"),
            "one\nmatch1\nthree\nmatch2\nfive\n",
        )
        .unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "match",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content",
                "context_lines": 1,
            }),
        )
        .await;
        // Match lines use ':' with line numbers; context lines use '-'.
        assert!(text.contains(": match1"));
        assert!(text.contains(": match2"));
        assert!(text.contains("- one"));
        assert!(text.contains("- three"));
        assert!(text.contains("- five"));
        // Overlapping contexts (line 3 is after match1 and before match2) merge: one region.
        assert_eq!(text.matches("three").count(), 1);
        assert!(!text.contains("--"));
        // File path printed once as header.
        assert_eq!(text.matches("a.txt").count(), 1);
    }

    #[tokio::test]
    async fn content_mode_separates_disjoint_regions() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a\nmatch1\nb\nc\nd\nmatch2\ne\n").unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "match",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content",
            }),
        )
        .await;
        assert!(text.contains(": match1"));
        assert!(text.contains(": match2"));
        assert_eq!(text.matches("--").count(), 1);
    }

    #[tokio::test]
    async fn files_with_matches_lists_paths_only() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello one\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello two\n").unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "hello",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "files_with_matches",
            }),
        )
        .await;
        let a = dir.path().join("a.txt").display().to_string();
        let b = dir.path().join("b.txt").display().to_string();
        assert!(text.contains(&a));
        assert!(text.contains(&b));
        assert!(!text.contains("hello one"));
        assert!(!text.contains("hello two"));
        assert_eq!(text.lines().count(), 2);
    }

    #[tokio::test]
    async fn count_mode_reports_per_file_counts() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\nhello\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "hi hello\n").unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "hello",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "count",
            }),
        )
        .await;
        let a = dir.path().join("a.txt").display().to_string();
        let b = dir.path().join("b.txt").display().to_string();
        assert!(text.contains(&format!("2\t{a}")));
        assert!(text.contains(&format!("1\t{b}")));
    }

    #[tokio::test]
    async fn max_results_truncates_content_output() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "m1\nm2\nm3\nm4\nm5\n").unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "^m",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content",
                "max_results": 2,
            }),
        )
        .await;
        assert!(text.contains("... (2 matches shown, may be more)"));
        assert!(text.contains(": m1"));
        assert!(text.contains(": m2"));
        assert!(!text.contains("m3"));
    }

    #[tokio::test]
    async fn invalid_output_mode_is_rejected() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();

        let tool = GrepTool;
        let r = tool
            .execute(
                "g",
                json!({
                    "pattern": "hello",
                    "path": dir.path().to_str().unwrap(),
                    "output_mode": "bogus",
                }),
                CancellationToken::new(),
                None,
            )
            .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn no_matches_reports_empty() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();

        let tool = GrepTool;
        let text = run(
            &tool,
            json!({
                "pattern": "zzz",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "count",
            }),
        )
        .await;
        assert!(text.contains("No matches"));
    }
}
