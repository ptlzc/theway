//! `grep` tool — line-based regex match across a directory tree. Models
//! `packages/coding-agent/src/core/tools/grep.ts` at a simplified level.
//!
//! Two execution paths share one formatting pipeline (issue #121):
//!   - **tgrep path** (default once available): when the daemon's
//!     [`TgrepServerRegistry`](crate::tgrep_server::TgrepServerRegistry) has a
//!     *ready* (index-complete) `tgrep serve` for the session root and the
//!     query path is inside it, the tool runs the `tgrep` client with
//!     `--json` and ingests the ripgrep-compatible NDJSON stream. Trigram
//!     index makes queries instant on large repos.
//!   - **walker path** (fallback): `ignore::WalkBuilder` + the `regex` crate,
//!     used while the serve index builds, when the binary is missing, or for
//!     paths outside the session root. Always complete — never partial-index
//!     results.
//!
//! `output_mode` mirrors the enhanced-tools `grep.ts`:
//!   - `content` (default): ripgrep-style grouped output — file path printed once, merged
//!     overlapping contexts, `--` between disjoint regions, `:` for match lines, `-` for
//!     context lines.
//!   - `files_with_matches`: just the file paths (deduped).
//!   - `count`: `count<TAB>path` per file (ripgrep `--count` style, matching lines).
//!
//! Result count is capped (`max_results`, matching lines); long lines are previewed around
//! the match with `[line truncated]` markers.
//!
//! Layout: one submodule per domain — `tgrep` (indexed NDJSON backend),
//! `walker` (in-process fallback backend), `ingest` (the accumulator both
//! paths fill), `format` (the three output modes), `preview` (long-line
//! previews).

use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use crate::tgrep_server::{TgrepReadiness, TgrepServerRegistry};

mod format;
mod ingest;
mod preview;
mod tgrep;
mod walker;

use self::format::{format_content, format_counts, format_files_with_matches};
use self::ingest::Ingest;
use self::tgrep::run_tgrep_client;
use self::walker::walk_tree;

// The inline unit suites (`tests`, `coverage_gap`) resolve these names through
// this module's scope via `use super::*`; production code imports them in the
// owning submodule directly, so the re-imports here are test-only.
#[cfg(test)]
use self::preview::{preview_match_line, truncate_line};
#[cfg(test)]
use self::tgrep::ingest_json_record;

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_MATCH_LINE_CHARS: usize = 500;
/// tgrep's on-disk index directory name (upstream constant).
const INDEX_DIR_NAME: &str = ".tgrep";

pub struct GrepTool {
    /// Daemon-wide serve registry; `None` = walker-only (tests, sandbox).
    tgrep: Option<TgrepServerRegistry>,
    /// The session root the registry may serve. Queries outside it never take
    /// the tgrep path (we don't index arbitrary directories).
    home_root: PathBuf,
}

impl GrepTool {
    pub fn new(tgrep: Option<TgrepServerRegistry>, home_root: PathBuf) -> Self {
        Self { tgrep, home_root }
    }
}

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
        let cwd = params.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
        let raw_path = params.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let path = if std::path::Path::new(raw_path).is_absolute() {
            raw_path.to_string()
        } else if raw_path == "." {
            cwd.to_string()
        } else {
            std::path::Path::new(cwd)
                .join(raw_path)
                .to_string_lossy()
                .into_owned()
        };
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

        // Run the whole decision + execution inside one spawn_blocking task:
        // the walker honors .gitignore without blocking the runtime, and the
        // tgrep path's readiness poll / client I/O is synchronous by design.
        let tgrep = self.tgrep.clone();
        let home_root = self.home_root.clone();
        let pattern_owned = pattern.to_string();
        let path_owned = path.clone();
        let glob = glob.map(str::to_string);
        let re_clone = re.clone();
        let cancel_clone = cancel.clone();
        let ingest = tokio::task::spawn_blocking(move || -> Result<Ingest, String> {
            // tgrep path eligibility: a ready serve for the home root and the
            // query path inside it. Anything else uses the walker.
            if let Some(registry) = tgrep.as_ref() {
                let home = home_root.canonicalize().ok();
                let target = Path::new(&path_owned).canonicalize().ok();
                let eligible = match (&home, &target) {
                    (Some(home), Some(target)) => target.starts_with(home),
                    _ => false,
                };
                if eligible {
                    let home = home.expect("eligible implies a canonicalized home root");
                    match registry.query_root(&home) {
                        TgrepReadiness::Ready => {
                            let binary = registry.binary_path();
                            let Some(binary) = binary else {
                                tracing::warn!(target: "tgrep", "ready serve but tgrep binary missing; falling back to the walker");
                                return Err("tgrep binary not found".to_string());
                            };
                            let index_dir = home.join(INDEX_DIR_NAME);
                            return run_tgrep_client(
                                &binary,
                                &index_dir,
                                &path_owned,
                                &pattern_owned,
                                &re_clone,
                                case_insensitive,
                                context_lines,
                                glob.as_deref(),
                                max_matches,
                                &cancel_clone,
                            );
                        }
                        readiness => {
                            // Missing/Indexing: walker path below (complete results).
                            tracing::debug!(target: "tgrep", ?readiness, "tgrep not ready; grep walks");
                        }
                    }
                }
            }
            walk_tree(
                &path_owned,
                &re_clone,
                context_lines,
                glob.as_deref(),
                max_matches,
                &cancel_clone,
            )
        })
        .await
        .map_err(|e| AgentToolError::from(format!("spawn_blocking: {e}")))?
        .map_err(AgentToolError::from)?;

        let output_mode = output_mode.unwrap_or("content");
        let (text, truncated_lines) = if ingest.match_count() == 0 {
            (format!("No matches for /{pattern}/ in {path}"), 0)
        } else {
            match output_mode {
                "files_with_matches" => (format_files_with_matches(&ingest), 0),
                "count" => (format_counts(&ingest), 0),
                _ => format_content(&ingest),
            }
        };

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(text)],
            details: json!({
                "matches": ingest.match_count(),
                "truncated_lines": truncated_lines,
                "max_match_line_chars": MAX_MATCH_LINE_CHARS,
                "outputMode": output_mode,
            }),
            terminate: None,
        })
    }
}

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
            "path": { "type": "string", "description": "Directory to search (default: current; relative paths resolve against cwd)" },
            "cwd": { "type": "string", "description": "Working directory for resolving a relative/default path (optional; defaults to the session cwd)" },
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

    fn walker_tool() -> GrepTool {
        GrepTool::new(None, PathBuf::from("."))
    }

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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

        let tool = walker_tool();
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

    /// JSON-stream ingestion must produce the same map/occurrences the walker
    /// path does for the same fixture (shared formatter => byte-identical).
    #[test]
    fn json_ingestion_matches_walker_semantics() {
        let re = Regex::new("hello").unwrap();
        let mut ingest = Ingest::new(100);
        let records = [
            r#"{"type":"begin","data":{"path":{"text":"a.txt"}}}"#,
            r#"{"type":"match","data":{"path":{"text":"a.txt"},"lines":{"text":"hello world\n"},"line_number":1,"submatches":[{"match":{"text":"hello"},"start":0,"end":5}]}}"#,
            r#"{"type":"context","data":{"path":{"text":"a.txt"},"lines":{"text":"foo bar\n"},"line_number":2,"submatches":[]}}"#,
            r#"{"type":"match","data":{"path":{"text":"b.txt"},"lines":{"text":"x hello hello y\n"},"line_number":7,"submatches":[{"match":{"text":"hello"},"start":2,"end":7},{"match":{"text":"hello"},"start":8,"end":13}]}}"#,
            r#"{"type":"end","data":{"path":{"text":"b.txt"}}}"#,
            r#"{"type":"summary","data":{"stats":{}}}"#,
        ];
        for line in records {
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            ingest_json_record(&mut ingest, &record, &re);
        }
        // One occurrence per matching line, regardless of submatch count.
        assert_eq!(ingest.match_count(), 2);
        assert_eq!(ingest.by_file.len(), 2);
        let b = &ingest.by_file["b.txt"];
        assert_eq!(b.len(), 1);
        assert_eq!(b[&7], ("x hello hello y".to_string(), true));
        // Context line lands as a non-match entry.
        let a = &ingest.by_file["a.txt"];
        assert_eq!(a[&2], ("foo bar".to_string(), false));
        // files_with_matches: deduped in first-seen order.
        assert_eq!(format_files_with_matches(&ingest), "a.txt\nb.txt");
        // count: matching lines per file.
        let counts = format_counts(&ingest);
        assert!(counts.contains("1\ta.txt"));
        assert!(counts.contains("1\tb.txt"));
    }

    #[test]
    fn json_ingestion_caps_at_max_matches() {
        let re = Regex::new("m").unwrap();
        let mut ingest = Ingest::new(2);
        let records = [
            r#"{"type":"match","data":{"path":{"text":"a.txt"},"lines":{"text":"m\n"},"line_number":1,"submatches":[{"match":{"text":"m"},"start":0,"end":1}]}}"#,
            r#"{"type":"match","data":{"path":{"text":"a.txt"},"lines":{"text":"m\n"},"line_number":2,"submatches":[{"match":{"text":"m"},"start":0,"end":1}]}}"#,
            r#"{"type":"match","data":{"path":{"text":"a.txt"},"lines":{"text":"m\n"},"line_number":3,"submatches":[{"match":{"text":"m"},"start":0,"end":1}]}}"#,
        ];
        for line in records {
            let record: serde_json::Value = serde_json::from_str(line).unwrap();
            ingest_json_record(&mut ingest, &record, &re);
            if ingest.is_capped() {
                break; // run_tgrep_client kills the child here
            }
        }
        assert_eq!(ingest.match_count(), 2);
        assert!(ingest.is_capped());
    }
}

#[cfg(test)]
mod coverage_gap {
    use super::*;
    use tempfile::tempdir;

    fn walker_tool() -> GrepTool {
        GrepTool::new(None, PathBuf::from("."))
    }

    async fn run(tool: &GrepTool, params: Value) -> String {
        let r = tool
            .execute("g", params, CancellationToken::new(), None)
            .await
            .unwrap();
        match &r.content[0] {
            UserContentBlock::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn missing_pattern_is_rejected() {
        let tool = walker_tool();
        let err = tool
            .execute("g", json!({}), CancellationToken::new(), None)
            .await
            .expect_err("missing pattern must fail");
        assert!(err.to_string().contains("missing `pattern`"), "got: {err}");
    }

    #[tokio::test]
    async fn invalid_regex_is_rejected() {
        let tool = walker_tool();
        let err = tool
            .execute(
                "g",
                json!({ "pattern": "[", "path": "." }),
                CancellationToken::new(),
                None,
            )
            .await
            .expect_err("invalid regex must fail");
        assert!(err.to_string().contains("regex"), "got: {err}");
    }

    #[tokio::test]
    async fn relative_path_resolves_against_cwd() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("a.txt"), "needle\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "needle\n").unwrap();

        let tool = walker_tool();
        let text = run(
            &tool,
            json!({
                "pattern": "needle",
                "cwd": dir.path().to_str().unwrap(),
                "path": "sub",
            }),
        )
        .await;
        assert!(text.contains("a.txt"), "got: {text}");
        assert!(!text.contains("b.txt"), "got: {text}");
    }

    #[test]
    fn preview_match_line_falls_back_to_head_when_no_range() {
        let line = "x".repeat(MAX_MATCH_LINE_CHARS + 100);
        let (preview, truncated) = preview_match_line(&line, None);
        assert!(truncated);
        assert!(preview.ends_with("...[line truncated]"), "got: {preview}");
        assert!(preview.starts_with(&"x".repeat(MAX_MATCH_LINE_CHARS)));
    }

    #[test]
    fn truncate_line_caps_long_context_lines() {
        let line = "y".repeat(MAX_MATCH_LINE_CHARS + 10);
        let truncated = truncate_line(&line);
        assert!(truncated.contains("[line truncated]"), "got: {truncated}");
    }

    #[test]
    fn format_files_with_matches_dedupes_in_scan_order() {
        let mut ingest = Ingest::new(10);
        ingest.match_line("a.txt", 1, "a", false);
        ingest.match_line("b.txt", 1, "b", false);
        ingest.match_line("a.txt", 2, "a2", false);
        assert_eq!(format_files_with_matches(&ingest), "a.txt\nb.txt");
    }

    #[test]
    fn truncate_line_short_line_stays_unchanged() {
        assert_eq!(truncate_line("short"), "short");
        assert_eq!(
            truncate_line(&"x".repeat(MAX_MATCH_LINE_CHARS)),
            "x".repeat(MAX_MATCH_LINE_CHARS)
        );
    }

    #[test]
    fn ingest_json_record_missing_fields_are_ignored() {
        let re = Regex::new("m").unwrap();
        let mut ingest = Ingest::new(10);
        for record in [
            r#"{"data":{}}"#,
            r#"{"type":"match","data":{}}"#,
            r#"{"type":"match","data":{"path":{"text":"a"} }}"#,
            r#"{"type":"match","data":{"path":{"text":"a"},"lines":{"text":"m\n"}}}"#,
        ] {
            let v: serde_json::Value = serde_json::from_str(record).unwrap();
            ingest_json_record(&mut ingest, &v, &re);
        }
        assert_eq!(ingest.match_count(), 0);
    }

    #[test]
    fn ingest_json_record_without_submatches_uses_regex_find() {
        let re = Regex::new("needle").unwrap();
        let mut ingest = Ingest::new(10);
        let record = serde_json::json!({
            "type": "match",
            "data": {
                "path": {"text": "a.txt"},
                "lines": {"text": "a needle here\n"},
                "line_number": 3
            }
        });
        ingest_json_record(&mut ingest, &record, &re);
        assert_eq!(ingest.match_count(), 1);
        assert_eq!(ingest.by_file["a.txt"][&3].0, "a needle here");
    }

    #[test]
    fn walk_tree_cancelled_returns_immediately() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let re = Regex::new("needle").unwrap();
        let ingest = walk_tree(dir.path().to_str().unwrap(), &re, 0, None, 10, &cancel).unwrap();
        assert_eq!(ingest.match_count(), 0);
    }

    #[test]
    fn walk_tree_invalid_glob_returns_error() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "needle\n").unwrap();
        let re = Regex::new("needle").unwrap();
        let err = match walk_tree(
            dir.path().to_str().unwrap(),
            &re,
            0,
            Some("["),
            10,
            &CancellationToken::new(),
        ) {
            Ok(_) => panic!("invalid glob should error"),
            Err(e) => e,
        };
        assert!(!err.is_empty(), "invalid glob should surface an error");
    }
}
