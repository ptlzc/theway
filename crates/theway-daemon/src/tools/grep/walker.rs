//! Walker fallback backend: `ignore::WalkBuilder` + the `regex` crate, used
//! while the tgrep index builds, when the binary is missing, or for paths
//! outside the session root.

use ignore::WalkBuilder;
use regex::Regex;
use tokio_util::sync::CancellationToken;

use super::ingest::Ingest;
use super::preview::{preview_match_line, truncate_line};

const DEFAULT_MAX_FILES: usize = 5_000;

/// The in-process walker fallback: `ignore::WalkBuilder` + the `regex` crate,
/// one occurrence per matching line (the pre-tgrep behavior, preserved).
pub(super) fn walk_tree(
    path: &str,
    re: &Regex,
    context_lines: usize,
    glob: Option<&str>,
    max_matches: usize,
    cancel: &CancellationToken,
) -> Result<Ingest, String> {
    let mut walker = WalkBuilder::new(path);
    walker.standard_filters(true).hidden(true);
    if let Some(g) = glob {
        let mut tb = ignore::types::TypesBuilder::new();
        tb.add("g", g).map_err(|e| e.to_string())?;
        tb.select("g");
        let types = tb.build().map_err(|e| e.to_string())?;
        walker.types(types);
    }
    let walker = walker.build();
    let mut ingest = Ingest::new(max_matches);
    let mut files_scanned = 0usize;
    for entry in walker {
        if cancel.is_cancelled() {
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
            if !re.is_match(line) {
                continue;
            }
            let lineno = i + 1;
            let before_start = i.saturating_sub(context_lines);
            let after_end = (i + 1 + context_lines).min(lines.len());
            for (j, ctx) in lines[before_start..i].iter().enumerate() {
                ingest.context_line(
                    &p.display().to_string(),
                    lineno - (i - before_start) + j,
                    truncate_line(ctx),
                );
            }
            let (preview, was_truncated) =
                preview_match_line(line, re.find(line).map(|m| (m.start(), m.end())));
            ingest.match_line(&p.display().to_string(), lineno, &preview, was_truncated);
            for (j, ctx) in lines[i + 1..after_end].iter().enumerate() {
                ingest.context_line(&p.display().to_string(), lineno + 1 + j, truncate_line(ctx));
            }
            if ingest.is_capped() {
                return Ok(ingest);
            }
        }
    }
    Ok(ingest)
}
