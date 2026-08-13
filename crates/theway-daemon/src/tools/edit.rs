//! `edit` tool — exact-string replacement. Models `packages/coding-agent/src/core/tools/edit.ts`
//! at a simplified level: read → require unique `old_string` (unless `replace_all`) → write
//! the new file. Reports a 3-line context diff in the result so the LLM sees what changed.
//!
//! Optional `range: [startLine, endLine]` (1-indexed, inclusive) restricts the search to a
//! line range; the entire match must lie within it. Duplicate matches produce a diagnostic
//! with per-occurrence line numbers + context (mirrors enhanced-tools `edit.ts`).
//!
//! Concurrency (issue #17): the read→modify→write cycle holds a cross-process
//! [`FileLock`](crate::executor::file_lock::FileLock) keyed on the target path, so
//! parallel agents (subagents) editing the same file serialize instead of silently
//! losing each other's edits. The lock is taken only when the file already exists —
//! an edit on a missing file fails at the read anyway, and locking must not create
//! empty files as a side effect.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::executor::ToolExecutor;
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

/// File read + write dispatch through the injected [`ToolExecutor`]
/// (sdk-split-local-sandbox node 8).
pub struct EditTool {
    executor: Arc<dyn ToolExecutor>,
}

impl EditTool {
    pub fn new(executor: Arc<dyn ToolExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl AgentTool for EditTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "edit"
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `path`"))?;
        let old = params
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `old_string`"))?;
        let new_ = params
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `new_string`"))?;
        let replace_all = params
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if old == new_ {
            return Err(AgentToolError::from(
                "old_string must differ from new_string",
            ));
        }
        if old.is_empty() {
            return Err(AgentToolError::from("old_string must not be empty"));
        }
        let raw_range: Option<(usize, usize)> = match params.get("range") {
            None => None,
            Some(Value::Array(arr)) if arr.len() == 2 => match (arr[0].as_u64(), arr[1].as_u64()) {
                (Some(s), Some(e)) => Some((s as usize, e as usize)),
                _ => {
                    return Err(AgentToolError::from(
                        "range must be [startLine, endLine] with integer values",
                    ));
                }
            },
            Some(_) => {
                return Err(AgentToolError::from(
                    "range must be an array of two integers: [startLine, endLine]",
                ));
            }
        };

        // Serialize concurrent editors of this file across processes (subagents
        // share one working tree). Held until the end of `execute`, i.e. across
        // the read AND the write.
        let _lock = if std::fs::metadata(Path::new(path)).is_ok() {
            Some(
                crate::executor::file_lock::FileLock::acquire(Path::new(path))
                    .await
                    .map_err(|e| AgentToolError::from(format!("lock {path} for editing: {e}")))?,
            )
        } else {
            None
        };

        let body = self
            .executor
            .read_file(Path::new(path))
            .await
            .map_err(|e| AgentToolError::from(format!("read {path}: {e}")))?;

        let line_starts = build_line_starts(&body);
        let total_lines = line_starts.len();

        // Byte range covering lines `start..=end` (inclusive); the whole match must fit.
        let (search_start, search_end, range_label) = match raw_range {
            None => (0usize, body.len(), String::new()),
            Some((start_line, end_line)) => {
                if start_line < 1
                    || start_line > total_lines
                    || end_line < start_line
                    || end_line > total_lines
                {
                    return Err(AgentToolError::from(format!(
                        "Invalid range [{start_line},{end_line}]: file has {total_lines} lines. \
                         Range must be [start,end] with 1 ≤ start ≤ end ≤ {total_lines}."
                    )));
                }
                let end = line_starts.get(end_line).copied().unwrap_or(body.len());
                (
                    line_starts[start_line - 1],
                    end,
                    format!(" within lines {start_line}-{end_line}"),
                )
            }
        };

        let occurrences = find_occurrences(&body, old, search_start, search_end);

        if occurrences.is_empty() {
            return Err(AgentToolError::from(format!(
                "old_string not found in {path}{range_label}."
            )));
        }
        if occurrences.len() > 1 && !replace_all {
            return Err(AgentToolError::from(build_duplicate_diagnostic(
                &body,
                &occurrences,
                path,
                &range_label,
            )));
        }

        let mut new_body = body.clone();
        if replace_all {
            for &pos in occurrences.iter().rev() {
                new_body.replace_range(pos..pos + old.len(), new_);
            }
        } else {
            let pos = occurrences[0];
            new_body.replace_range(pos..pos + old.len(), new_);
        }
        self.executor
            .write_file(Path::new(path), &new_body)
            .await
            .map_err(|e| AgentToolError::from(format!("write {path}: {e}")))?;

        let count = occurrences.len();
        let preview = render_diff_preview(old, new_);
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "Edited {path} ({count} replacement{}){range_label}.\n{preview}",
                if count == 1 { "" } else { "s" },
            ))],
            details: json!({
                "path": path,
                "replacements": count,
                "replaceAll": replace_all,
            }),
            terminate: None,
        })
    }
}
/// Line-start byte offsets. `line_starts[k]` is the offset of line `k+1`; the last entry is
/// the offset just past the final newline (== `body.len()` for files ending in `\n`), so
/// `line_starts.len()` equals the number of lines (`split("\n").length` in the TS model).
fn build_line_starts(body: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in body.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// 1-indexed line number for a byte offset (binary search over `build_line_starts`).
fn line_at(line_starts: &[usize], pos: usize) -> usize {
    let mut lo = 0usize;
    let mut hi = line_starts.len() - 1;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if line_starts[mid] <= pos {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo + 1
}

/// Find all (non-overlapping) byte offsets of `old` within `[search_start, search_end)`.
/// A match that starts inside but extends past `search_end` is rejected, so the entire
/// `old_string` must lie within the search range.
fn find_occurrences(body: &str, old: &str, search_start: usize, search_end: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = search_start;
    while from < search_end {
        let Some(rel) = body.get(from..).and_then(|s| s.find(old)) else {
            break;
        };
        let pos = from + rel;
        if pos + old.len() > search_end {
            break;
        }
        out.push(pos);
        from = pos + old.len();
    }
    out
}

/// Diagnostic for a non-unique `old_string`: per-occurrence line number + 2 lines of context
/// (first 10 occurrences), mirroring enhanced-tools `edit.ts`.
fn build_duplicate_diagnostic(
    body: &str,
    occurrences: &[usize],
    path: &str,
    range_label: &str,
) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let line_starts = build_line_starts(body);
    let mut msg = format!(
        "old_string is not unique in {path}{range_label}. Found {} occurrences:\n\n",
        occurrences.len()
    );
    for (i, &pos) in occurrences.iter().enumerate() {
        if i >= 10 {
            msg.push_str(&format!(
                "\n... and {} more occurrences (not shown).\n",
                occurrences.len() - 10
            ));
            break;
        }
        let line = line_at(&line_starts, pos);
        msg.push_str(&format!("Occurrence {} at line {line}:\n", i + 1));
        let ctx_start = line.saturating_sub(3);
        let ctx_end = (line + 2).min(lines.len());
        for (j, l) in lines[ctx_start..ctx_end].iter().enumerate() {
            msg.push_str(&format!("{:>4}| {l}\n", ctx_start + j + 1));
        }
        if i < occurrences.len() - 1 && i < 9 {
            msg.push_str("\n---\n\n");
        }
    }
    msg.push_str(
        "\n\nProvide more context to make old_string unique, use range=[startLine,endLine] to \
         restrict scope, or use replace_all=true.",
    );
    msg
}

/// Render a minimal 3-context-line diff of the changed region. We don't have a real diff
/// algorithm here; we just show the old vs new strings labeled — sufficient for the LLM to
/// confirm the edit landed.
fn render_diff_preview(old: &str, new_: &str) -> String {
    let mut s = String::from("--- before\n");
    for line in old.lines().take(10) {
        s.push_str("- ");
        s.push_str(line);
        s.push('\n');
    }
    s.push_str("+++ after\n");
    for line in new_.lines().take(10) {
        s.push_str("+ ");
        s.push_str(line);
        s.push('\n');
    }
    s
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "edit".into(),
    description:
        "Replace an exact substring in a file. The substring must be unique within the file \
         (or the optional range) unless `replace_all` is true. Fails with a diagnostic listing \
         occurrence line numbers + context when not unique. Use `read` first to confirm the exact \
         text to match, including surrounding context; use range=[startLine,endLine] to restrict \
         the search to a line range."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to the file (relative or absolute)" },
            "old_string": { "type": "string", "description": "Exact substring to replace. Include enough surrounding context to make it unique within the file (or the given range)." },
            "new_string": { "type": "string", "description": "Replacement string. Use the empty string to delete." },
            "replace_all": { "type": "boolean", "description": "Replace every occurrence rather than requiring uniqueness." },
            "range": { "type": "array", "minItems": 2, "maxItems": 2, "items": { "type": "integer" }, "description": "Optional [startLine, endLine] (1-indexed, inclusive) to restrict search/replace to within this line range. Default: entire file. The entire match must lie within the range." },
        },
        "required": ["path", "old_string", "new_string"],
    }),
});
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn local_exec() -> Arc<dyn ToolExecutor> {
        Arc::new(crate::executor::local::LocalExecutor::new())
    }

    #[tokio::test]
    async fn replaces_unique_substring() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "hello world\nfoo bar\n").unwrap();
        let tool = EditTool::new(local_exec());
        tool.execute(
            "e",
            json!({ "path": p.to_str().unwrap(), "old_string": "hello", "new_string": "hey" }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body, "hey world\nfoo bar\n");
    }

    #[tokio::test]
    async fn rejects_ambiguous_match() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "foo\nfoo\n").unwrap();
        let tool = EditTool::new(local_exec());
        let r = tool
            .execute(
                "e",
                json!({ "path": p.to_str().unwrap(), "old_string": "foo", "new_string": "bar" }),
                CancellationToken::new(),
                None,
            )
            .await;
        let err = format!("{}", r.unwrap_err());
        assert!(err.contains("Occurrence 1 at line 1"), "{err}");
        assert!(err.contains("Occurrence 2 at line 2"), "{err}");
    }

    #[tokio::test]
    async fn replace_all_handles_multiple() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "foo\nfoo\nfoo\n").unwrap();
        let tool = EditTool::new(local_exec());
        tool.execute(
            "e",
            json!({
                "path": p.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar",
                "replace_all": true,
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "bar\nbar\nbar\n");
    }

    #[tokio::test]
    async fn range_restricts_replacement() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "foo\nfoo\nfoo\n").unwrap();
        let tool = EditTool::new(local_exec());
        tool.execute(
            "e",
            json!({
                "path": p.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar",
                "range": [2, 2],
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "foo\nbar\nfoo\n");
    }

    #[tokio::test]
    async fn range_outside_match_is_not_found() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "needle\nother\nother\n").unwrap();
        let tool = EditTool::new(local_exec());
        let r = tool
            .execute(
                "e",
                json!({
                    "path": p.to_str().unwrap(),
                    "old_string": "needle",
                    "new_string": "x",
                    "range": [2, 3],
                }),
                CancellationToken::new(),
                None,
            )
            .await;
        let err = format!("{}", r.unwrap_err());
        assert!(err.contains("not found"), "{err}");
        assert!(err.contains("within lines 2-3"), "{err}");
    }

    #[tokio::test]
    async fn multiline_old_string_within_range() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "a\nb\nc\nd\n").unwrap();
        let tool = EditTool::new(local_exec());
        tool.execute(
            "e",
            json!({
                "path": p.to_str().unwrap(),
                "old_string": "b\nc",
                "new_string": "X",
                "range": [2, 3],
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "a\nX\nd\n");
    }

    #[tokio::test]
    async fn multiline_old_string_straddling_range_end_not_found() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "a\nb\nc\nd\n").unwrap();
        let tool = EditTool::new(local_exec());
        let r = tool
            .execute(
                "e",
                json!({
                    "path": p.to_str().unwrap(),
                    "old_string": "b\nc",
                    "new_string": "X",
                    "range": [2, 2],
                }),
                CancellationToken::new(),
                None,
            )
            .await;
        let err = format!("{}", r.unwrap_err());
        assert!(err.contains("not found"), "{err}");
    }

    #[tokio::test]
    async fn replace_all_within_range() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "foo\nfoo\nfoo\n").unwrap();
        let tool = EditTool::new(local_exec());
        tool.execute(
            "e",
            json!({
                "path": p.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "bar",
                "replace_all": true,
                "range": [2, 3],
            }),
            CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "foo\nbar\nbar\n");
    }

    #[tokio::test]
    async fn invalid_range_rejected() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, "foo\nfoo\nfoo\n").unwrap();
        let tool = EditTool::new(local_exec());
        for range in [[2, 1], [0, 2], [2, 9]] {
            let r = tool
                .execute(
                    "e",
                    json!({
                        "path": p.to_str().unwrap(),
                        "old_string": "foo",
                        "new_string": "bar",
                        "range": range,
                    }),
                    CancellationToken::new(),
                    None,
                )
                .await;
            let err = format!("{}", r.unwrap_err());
            assert!(err.contains("Invalid range"), "{err}");
        }
    }

    /// Issue #17 regression: parallel editors on one file must serialize on
    /// the file lock — every edit lands, none is silently clobbered by a
    /// last-writer-wins race. Without the lock the tasks read the same body
    /// and only the final write survives.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn concurrent_edits_to_same_file_all_land() {
        const N: usize = 8;
        let dir = tempdir().unwrap();
        let p = dir.path().join("slots.txt");
        let mut body = String::new();
        for i in 0..N {
            body.push_str(&format!("<!-- SLOT{i} -->\n"));
        }
        std::fs::write(&p, &body).unwrap();

        let tool = Arc::new(EditTool::new(local_exec()));
        let mut handles = Vec::new();
        for i in 0..N {
            let tool = tool.clone();
            let p = p.clone();
            handles.push(tokio::spawn(async move {
                tool.execute(
                    "e",
                    json!({
                        "path": p.to_str().unwrap(),
                        "old_string": format!("<!-- SLOT{i} -->"),
                        "new_string": format!("<!-- SLOT{i} DONE -->"),
                    }),
                    CancellationToken::new(),
                    None,
                )
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let final_body = std::fs::read_to_string(&p).unwrap();
        for i in 0..N {
            assert!(
                final_body.contains(&format!("<!-- SLOT{i} DONE -->")),
                "edit {i} was lost: {final_body}"
            );
        }
    }
}
