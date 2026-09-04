//! Official theway runtime extension packages embedded as build-time data.
//!
//! The plugin ABI is unversioned (see `docs/extensions.md`), so extension
//! packages must ship with a matching daemon. This crate bundles the official
//! packages as `include_str!` data — no runtime dependencies — and lets the
//! daemon materialize the shipped ones into the managed extensions layer
//! (`<base>/extensions-managed/`), which the package catalog discovers
//! automatically and trusts without a user record.
//!
//! Package sources live under `packages/<extension-id>/` and stay the single
//! source of truth: the embedded content is whatever the build saw.

use std::path::Path;

/// One embedded extension package: its manifest id plus every file that makes
/// up the package directory (relative path → content).
pub struct EmbeddedPackage {
    pub id: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// The TUI-docs pointer package: tells the model where the theway
/// configuration guide lives. Shipped with every install method.
pub const TUI_DOCS: EmbeddedPackage = EmbeddedPackage {
    id: "tui-docs",
    files: &[
        (
            "theway-extension.json",
            include_str!("../packages/tui-docs/theway-extension.json"),
        ),
        ("index.js", include_str!("../packages/tui-docs/index.js")),
    ],
};

/// The DeepSeek Anchor reference package (`zeroAnchor: true` makes it inert
/// until explicitly enabled). Documented as a reference implementation; NOT
/// shipped into the managed layer by default.
pub const DEEPSEEK_ANCHOR: EmbeddedPackage = EmbeddedPackage {
    id: "deepseek-anchor",
    files: &[
        (
            "theway-extension.json",
            include_str!("../packages/deepseek-anchor/theway-extension.json"),
        ),
        (
            "index.js",
            include_str!("../packages/deepseek-anchor/index.js"),
        ),
        (
            "anchor-config.json",
            include_str!("../packages/deepseek-anchor/anchor-config.json"),
        ),
        (
            "anchor-config.schema.json",
            include_str!("../packages/deepseek-anchor/anchor-config.schema.json"),
        ),
    ],
};

/// Packages provisioned into the managed layer at daemon startup.
pub const SHIPPED_PACKAGES: &[&EmbeddedPackage] = &[&TUI_DOCS];

/// Every embedded package (shipped + reference), for tests and tooling.
pub const ALL_PACKAGES: &[&EmbeddedPackage] = &[&TUI_DOCS, &DEEPSEEK_ANCHOR];

/// Provision the shipped packages into `<base>/extensions-managed/` when
/// missing or stale. Best-effort: failures are returned as warning strings —
/// the crate stays dependency-free, and the caller decides how to log. A
/// missing managed copy only means the pointer package stays absent.
///
/// Each package directory is written atomically: files land in a temporary
/// directory that is renamed over the target, so a crash mid-write never
/// leaves a half-written package for the catalog to discover.
pub fn ensure_managed_installed(base: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    for package in SHIPPED_PACKAGES {
        let target = base.join("extensions-managed").join(package.id);
        if up_to_date(&target, package.files) {
            continue;
        }
        let staging = base
            .join("extensions-managed")
            .join(format!(".{}-staging", package.id));
        let write = || -> std::io::Result<()> {
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging)?;
            for (relative, content) in package.files {
                let path = staging.join(relative);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, content)?;
            }
            let _ = std::fs::remove_dir_all(&target);
            std::fs::rename(&staging, &target)
        };
        if let Err(error) = write() {
            warnings.push(format!(
                "materializing managed package {} failed: {error}",
                package.id
            ));
        }
    }
    warnings
}

/// True when every embedded file exists with identical content.
fn up_to_date(target: &Path, files: &[(&str, &str)]) -> bool {
    files.iter().all(|(relative, content)| {
        std::fs::read_to_string(target.join(relative))
            .map(|existing| existing == *content)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_packages_are_complete_and_manifests_parse() {
        // Compile-time guard: the shipped list must be non-empty.
        const _: () = assert!(!SHIPPED_PACKAGES.is_empty());
        for package in ALL_PACKAGES {
            assert!(!package.files.is_empty(), "{} must embed files", package.id);
            let manifest = package
                .files
                .iter()
                .find(|(name, _)| *name == "theway-extension.json")
                .unwrap_or_else(|| panic!("{} lacks a manifest", package.id));
            let value: serde_json::Value =
                serde_json::from_str(manifest.1).expect("manifest must be valid JSON");
            assert_eq!(
                value["id"].as_str().unwrap(),
                package.id,
                "manifest id must match the package id"
            );
            let entry = value["entry"].as_str().unwrap();
            assert!(
                package.files.iter().any(|(name, _)| *name == entry),
                "{} entry file {} must be embedded",
                package.id,
                entry
            );
        }
    }

    #[test]
    fn materializes_missing_packages_and_refreshes_stale_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("extensions-managed").join("tui-docs");
        assert!(!target.exists());

        ensure_managed_installed(dir.path());
        assert_eq!(
            std::fs::read_to_string(target.join("index.js")).unwrap(),
            TUI_DOCS.files[1].1,
            "missing package must be materialized"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("theway-extension.json")).unwrap(),
            TUI_DOCS.files[0].1
        );

        // Idempotent: up-to-date content is not rewritten.
        let before = std::fs::metadata(target.join("index.js"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_managed_installed(dir.path());
        let after = std::fs::metadata(target.join("index.js"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "up-to-date package must not be rewritten");

        // Stale content is refreshed; unrelated user files in the managed
        // target directory are replaced by the canonical package.
        std::fs::write(target.join("index.js"), "stale").unwrap();
        std::fs::write(target.join("user-extra.js"), "extra").unwrap();
        ensure_managed_installed(dir.path());
        assert_eq!(
            std::fs::read_to_string(target.join("index.js")).unwrap(),
            TUI_DOCS.files[1].1,
            "stale package must be refreshed"
        );
        assert!(
            !target.join("user-extra.js").exists(),
            "the canonical package replaces the whole directory"
        );

        // No staging leftovers.
        let managed = dir.path().join("extensions-managed");
        let leftovers: Vec<_> = std::fs::read_dir(&managed)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "no staging directories may remain");
    }
}
