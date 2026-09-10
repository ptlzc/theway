//! tgrep backend. Query a `tgrep serve` instance with `--json` and ingest the
//! ripgrep-compatible NDJSON stream.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::ingest::Ingest;
use super::preview::{preview_match_line, truncate_line};

/// Query a `tgrep serve` instance with `--json` and ingest the NDJSON stream.
///
/// The client is pointed at the session root's index dir (`--index-path`) so
/// subdirectory queries still hit the server; `-s` pins case-sensitive
/// defaults (tgrep otherwise applies ripgrep-style smart-casing). Reading
/// stops as soon as `max_matches` matching lines are ingested, and the client
/// is killed mid-stream.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_tgrep_client(
    binary: &Path,
    index_dir: &Path,
    path: &str,
    pattern: &str,
    re: &Regex,
    case_insensitive: bool,
    context_lines: usize,
    glob: Option<&str>,
    max_matches: usize,
    cancel: &CancellationToken,
) -> Result<Ingest, String> {
    let mut cmd = Command::new(binary);
    cmd.arg("--index-path")
        .arg(index_dir)
        .arg("--json")
        .arg("--no-messages");
    cmd.arg(if case_insensitive { "-i" } else { "-s" });
    if context_lines > 0 {
        cmd.arg("-C").arg(context_lines.to_string());
    }
    if let Some(g) = glob {
        cmd.arg("-g").arg(g);
    }
    cmd.arg("-e").arg(pattern).arg(path);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn tgrep: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "tgrep stdout unavailable".to_string())?;
    let mut ingest = Ingest::new(max_matches);
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        if cancel.is_cancelled() || ingest.is_capped() {
            let _ = child.kill();
            break;
        }
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        ingest_json_record(&mut ingest, &record, re);
    }
    let _ = child.wait();
    Ok(ingest)
}

/// Ingest one NDJSON record (ripgrep-compatible). A match record is one
/// matching line (tgrep emits one per line, submatches listed), mirroring the
/// walker's per-line semantics; the preview window centers on the first
/// submatch byte range when present.
pub(super) fn ingest_json_record(ingest: &mut Ingest, record: &serde_json::Value, re: &Regex) {
    let Some(record_type) = record.get("type").and_then(|t| t.as_str()) else {
        return;
    };
    let Some(data) = record.get("data") else {
        return;
    };
    let Some(path) = data
        .get("path")
        .and_then(|p| p.get("text"))
        .and_then(|t| t.as_str())
    else {
        return;
    };
    let Some(line_text) = data
        .get("lines")
        .and_then(|l| l.get("text"))
        .and_then(|t| t.as_str())
    else {
        return;
    };
    let Some(lineno) = data.get("line_number").and_then(|n| n.as_u64()) else {
        return;
    };
    let lineno = lineno as usize;
    let line_text = line_text.strip_suffix('\n').unwrap_or(line_text);
    match record_type {
        "match" => {
            let range = data
                .get("submatches")
                .and_then(|s| s.as_array())
                .and_then(|spans| spans.first())
                .and_then(|span| {
                    Some((
                        span.get("start")?.as_u64()? as usize,
                        span.get("end")?.as_u64()? as usize,
                    ))
                })
                .or_else(|| re.find(line_text).map(|m| (m.start(), m.end())));
            let (preview, was_truncated) = preview_match_line(line_text, range);
            ingest.match_line(path, lineno, &preview, was_truncated);
        }
        "context" => ingest.context_line(path, lineno, truncate_line(line_text)),
        _ => {} // begin / end / summary carry no line data
    }
}
