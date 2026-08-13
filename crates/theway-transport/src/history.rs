//! shared client contract (not protocol) — zone per the crate-level "Module zones" doc.
//! Persistent input history for the REPL. Each submitted prompt is appended to
//! `~/.theway/history`, capped at 1000 entries. Subsequent sessions load it; the existing
//! line-based REPL doesn't yet offer ↑/↓ recall (needs raw mode → c4pt0r/theway#2 main
//! deliverable), but `/history` exposes the list and saved state is ready for the renderer.

use std::path::{Path, PathBuf};

use crate::client::base_dir;

const MAX_ENTRIES: usize = 1000;

pub struct HistoryStore {
    path: PathBuf,
    entries: Vec<String>,
}

impl HistoryStore {
    pub fn load() -> Self {
        Self::load_from(&Self::default_path())
    }

    pub fn default_path() -> PathBuf {
        base_dir().join("history")
    }

    pub fn load_from(path: &Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let entries: Vec<String> = text
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string())
            .collect();
        Self {
            path: path.to_path_buf(),
            entries,
        }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a fresh entry. Deduplicates with the immediately-preceding entry so spamming
    /// the same prompt twice doesn't litter the file. Caps total entries at MAX_ENTRIES
    /// (oldest dropped). Persists synchronously to the on-disk file.
    pub fn append(&mut self, prompt: &str) {
        let trimmed = prompt.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.entries.last().map(|s| s.as_str()) == Some(trimmed) {
            return;
        }
        self.entries.push(trimmed.to_string());
        if self.entries.len() > MAX_ENTRIES {
            let overflow = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..overflow);
        }
        let _ = self.save();
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = self.entries.join("\n") + "\n";
        std::fs::write(&self.path, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn append_persists_and_dedupes_adjacent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        let mut h = HistoryStore::load_from(&path);
        h.append("first");
        h.append("first"); // duplicate of immediate predecessor — should not store
        h.append("second");
        h.append("third");
        assert_eq!(h.entries, vec!["first", "second", "third"]);

        // Reload and verify on-disk state matches.
        let reloaded = HistoryStore::load_from(&path);
        assert_eq!(reloaded.entries, vec!["first", "second", "third"]);
    }

    #[test]
    fn cap_at_max_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        let mut h = HistoryStore::load_from(&path);
        for i in 0..(MAX_ENTRIES + 50) {
            h.append(&format!("entry-{i}"));
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(
            h.entries.first().map(|s| s.as_str()),
            Some(format!("entry-{}", 50).as_str())
        );
    }

    #[test]
    fn empty_and_whitespace_prompts_are_skipped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("history");
        let mut h = HistoryStore::load_from(&path);
        h.append("");
        h.append("   ");
        h.append("\t\n");
        assert!(h.entries.is_empty());
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = TempDir::new().unwrap();
        let h = HistoryStore::load_from(&dir.path().join("nope"));
        assert!(h.is_empty());
    }
}
