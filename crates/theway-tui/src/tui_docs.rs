//! Installed theway configuration documentation (issue #90): the `theway`
//! binary bundles the LLM-facing config guide and materializes it to
//! `<base>/docs/tui.md` on startup, so every install method
//! (`cargo install`, `scripts/install.sh`) carries it. The `tui-docs`
//! extension package points the model's prompt at this path instead of
//! injecting the content.
//!
//! The bundled copy is the single source: when the doc changes, the next
//! build ships it and the next startup refreshes the installed copy.

use std::path::Path;

/// Bundled TUI documentation, compiled into the binary at build time.
pub const TUI_DOCS_CONTENT: &str = include_str!("../docs/theway-config.md");

/// File name under `<base>/docs/`.
pub const TUI_DOCS_FILE: &str = "tui.md";

/// Materialize the bundled TUI documentation to `<base>/docs/tui.md` when it
/// is missing or differs from the bundled version. Best-effort: failures warn
/// on stderr and never block startup.
pub fn ensure_installed(base: &Path) {
    let target = base.join("docs").join(TUI_DOCS_FILE);
    let up_to_date = std::fs::read_to_string(&target)
        .map(|existing| existing == TUI_DOCS_CONTENT)
        .unwrap_or(false);
    if up_to_date {
        return;
    }
    let Some(parent) = target.parent() else {
        return;
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(parent)?;
        std::fs::write(&target, TUI_DOCS_CONTENT)
    };
    if let Err(error) = write() {
        eprintln!(
            "theway: writing bundled TUI docs to {} failed: {error}",
            target.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materializes_when_missing_and_refreshes_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("docs").join(TUI_DOCS_FILE);
        assert!(!target.exists());

        ensure_installed(dir.path());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            TUI_DOCS_CONTENT,
            "missing file must be materialized with the bundled content"
        );

        // Up-to-date content: the file is left untouched.
        let before = std::fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_installed(dir.path());
        let after = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(before, after, "up-to-date copy must not be rewritten");

        // Stale content: refreshed from the bundle.
        std::fs::write(&target, "stale").unwrap();
        ensure_installed(dir.path());
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            TUI_DOCS_CONTENT,
            "stale copy must be refreshed"
        );
    }
}
