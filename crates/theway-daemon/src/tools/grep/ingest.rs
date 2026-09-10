//! Shared ingestion for both execution paths. `by_file` feeds `content` mode
//! (per-line map dedups overlapping contexts); the occurrence list feeds
//! `count` / `files_with_matches` and the match cap (one entry per matching
//! line, mirroring the walker's semantics).

use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Ingest {
    pub(super) by_file: BTreeMap<String, BTreeMap<usize, (String, bool)>>,
    pub(super) occurrences: Vec<String>,
    pub(super) truncated_lines: usize,
    pub(super) max_matches: usize,
}

impl Ingest {
    pub(super) fn new(max_matches: usize) -> Self {
        Self {
            max_matches,
            ..Default::default()
        }
    }

    pub(super) fn match_line(
        &mut self,
        path: &str,
        lineno: usize,
        preview: &str,
        was_truncated: bool,
    ) {
        self.by_file
            .entry(path.to_string())
            .or_default()
            .insert(lineno, (preview.to_string(), true));
        self.occurrences.push(path.to_string());
        if was_truncated {
            self.truncated_lines += 1;
        }
    }

    pub(super) fn context_line(&mut self, path: &str, lineno: usize, text: String) {
        self.by_file
            .entry(path.to_string())
            .or_default()
            .entry(lineno)
            .or_insert((text, false));
    }

    pub(super) fn match_count(&self) -> usize {
        self.occurrences.len()
    }

    pub(super) fn is_capped(&self) -> bool {
        self.match_count() >= self.max_matches
    }
}
