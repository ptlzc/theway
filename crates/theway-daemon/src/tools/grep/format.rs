//! `grep` output modes. All three consume the same [`Ingest`] accumulator, so
//! the tgrep path and the walker path format byte-identically.

use super::MAX_MATCH_LINE_CHARS;
use super::ingest::Ingest;

/// `files_with_matches` mode: deduped file paths, one per line, in scan order.
pub(super) fn format_files_with_matches(ingest: &Ingest) -> String {
    let mut files: Vec<&str> = Vec::new();
    for path in &ingest.occurrences {
        if !files.contains(&path.as_str()) {
            files.push(path.as_str());
        }
    }
    files.join("\n")
}

/// `count` mode: `count<TAB>path` per file, ripgrep `--count` style
/// (matching lines per file).
pub(super) fn format_counts(ingest: &Ingest) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for path in &ingest.occurrences {
        match counts.iter_mut().find(|(p, _)| *p == path.as_str()) {
            Some((_, c)) => *c += 1,
            None => counts.push((path.as_str(), 1)),
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
pub(super) fn format_content(ingest: &Ingest) -> (String, usize) {
    let mut out: Vec<String> = Vec::new();
    for (file_path, line_map) in &ingest.by_file {
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
    if ingest.match_count() >= ingest.max_matches {
        text.push_str(&format!(
            "... ({} matches shown, may be more)\n",
            ingest.max_matches
        ));
    }
    text.push_str(out.join("\n").trim_end());
    if ingest.truncated_lines > 0 {
        text.push_str(&format!(
            "\n[{} long matching line(s) truncated to {MAX_MATCH_LINE_CHARS} chars]\n",
            ingest.truncated_lines
        ));
    }
    (text, ingest.truncated_lines)
}
