//! Controller-side prompt-template discovery (issue #96): the TUI owns local
//! template scanning and provisions the daemon with the scanned catalog through
//! the settings surface, so a controller-provisioned daemon never reads
//! template files itself.
//!
//! Root order and same-name-replacement semantics mirror the daemon's
//! `crate::templates::load_all`: roots are `[<base>/templates` (user),
//! `<work_dir>/.theway/templates` (project)] and a project entry replaces a
//! user entry of the same name. The walk rules mirror
//! `crate::templates::load_templates`: only root-level `*.md` files, no
//! recursion, dotfiles are NOT skipped, a missing frontmatter `name` falls back
//! to the file stem, and a missing/empty `description` is kept as an empty
//! string (the daemon stores `Option<String>` and does not skip such files —
//! the wire type uses `""` for "none").

use std::path::{Path, PathBuf};

use serde::Deserialize;
use theway_transport::wire::WireProvisionedTemplate;

/// Frontmatter shape parsed off the template file head — mirrors the daemon's
/// `crate::templates::TemplateFrontmatter` exactly (`name`, `description`).
#[derive(Clone, Debug, Default, Deserialize)]
struct TemplateFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Scan the local template roots and return the replace-on-collision catalog.
///
/// Roots, in precedence order: `base/templates` (user) then
/// `work_dir/.theway/templates` (project). Mirrors the daemon's `load_all`
/// loop: user templates are loaded first, then project templates replace any
/// same-named entry (project wins).
///
/// Wired into the settings payload by a later openspec node (config_payload.rs).
#[allow(dead_code)] // used by the config-payload wiring node (issue #96 follow-up)
pub(crate) fn scan_templates(work_dir: &Path, base: &Path) -> Vec<WireProvisionedTemplate> {
    let roots = [
        base.join("templates"),
        work_dir.join(".theway").join("templates"),
    ];

    let mut catalog: Vec<WireProvisionedTemplate> = Vec::new();
    for root in roots {
        load_dir(&root, &mut catalog);
    }
    catalog
}

/// Load root-level `*.md` files from one template dir. Missing/unreadable dirs
/// are silently skipped (same posture as the daemon's `load_templates`, which
/// only emits a diagnostic — the TUI catalog surface has no diagnostic field).
/// Entries are sorted so the catalog order is deterministic; the daemon's
/// `list_dir` order is unspecified, and ordering never affects the
/// replace-on-collision semantics.
fn load_dir(dir: &Path, catalog: &mut Vec<WireProvisionedTemplate>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // missing root is the documented default
    };
    let mut sorted: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    sorted.sort();
    for entry in sorted {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Mirrors the daemon's case-sensitive `entry.name.ends_with(".md")`
        // check (a `*.MD` file is NOT a template).
        if !name.ends_with(".md") {
            continue;
        }
        // The daemon takes `symlink_metadata`-based `FileKind::File`, so a
        // symlink to a `.md` file is skipped, exactly like a directory.
        let is_file =
            std::fs::symlink_metadata(&entry).is_ok_and(|metadata| metadata.file_type().is_file());
        if !is_file {
            continue;
        }
        if let Some(template) = load_template_file(&entry) {
            push_replacing(catalog, template);
        }
    }
}

fn load_template_file(file_path: &Path) -> Option<WireProvisionedTemplate> {
    let raw = std::fs::read_to_string(file_path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&raw).ok()?;
    let file_name = file_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = file_name
        .strip_suffix(".md")
        .unwrap_or(&file_name)
        .to_string();
    // Mirrors the daemon: `frontmatter.name.unwrap_or(stem)` — even an
    // explicitly empty `name:` wins over the stem, exactly like the daemon.
    let name = frontmatter.name.unwrap_or(stem);
    // Mirrors the daemon's `PromptTemplate.description: Option<String>`: a
    // missing/empty description is kept (not skipped); `""` is the wire
    // encoding of "none".
    let description = frontmatter.description.unwrap_or_default();
    Some(WireProvisionedTemplate {
        name,
        description,
        content: body,
        file_path: file_path.to_string_lossy().into_owned(),
    })
}

/// Mirrors `load_all`'s position-replace: a later (project) entry with the
/// same name overwrites the earlier (user) entry in place, preserving position.
fn push_replacing(catalog: &mut Vec<WireProvisionedTemplate>, template: WireProvisionedTemplate) {
    if let Some(i) = catalog
        .iter()
        .position(|existing| existing.name == template.name)
    {
        catalog[i] = template;
    } else {
        catalog.push(template);
    }
}

/// Parse the YAML frontmatter head — a byte-for-byte mirror of the daemon's
/// `crate::templates::parse_frontmatter`:
/// * no leading `---` → default frontmatter + the whole (normalized) body,
///   untrimmed;
/// * leading `---` but no closing `\n---` → same as no frontmatter;
/// * otherwise the YAML between the fences is parsed and the body is trimmed.
fn parse_frontmatter(content: &str) -> Result<(TemplateFrontmatter, String), String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok((TemplateFrontmatter::default(), normalized));
    }
    let Some(end) = normalized[3..].find("\n---") else {
        return Ok((TemplateFrontmatter::default(), normalized));
    };
    let end = end + 3;
    // Defensive divergence: an empty frontmatter (`---\n---`) would make the
    // daemon's `normalized[4..end]` slice start past `end` and panic. The
    // daemon's arithmetic has the same latent edge; we treat it as a parse
    // error so the template is skipped instead of crashing the TUI.
    let yaml = normalized.get(4..end).ok_or("yaml: empty frontmatter")?;
    let body = normalized[end + 4..].trim().to_string();
    let frontmatter = serde_yaml::from_str(yaml).map_err(|error| format!("yaml: {error}"))?;
    Ok((frontmatter, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn scans_flat_root_md_with_frontmatter() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/review.md"),
            "---\nname: review\ndescription: code review checklist\n---\nBody {{var}}\n",
        );

        let templates = scan_templates(&work_dir, &base);
        assert_eq!(templates.len(), 1, "{templates:?}");
        let template = &templates[0];
        assert_eq!(template.name, "review");
        assert_eq!(template.description, "code review checklist");
        assert_eq!(template.content, "Body {{var}}");
        assert!(
            template.file_path.ends_with("templates/review.md"),
            "{}",
            template.file_path
        );
    }

    #[test]
    fn project_replaces_user_on_same_name() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/review.md"),
            "---\nname: review\ndescription: user desc\n---\nuser body",
        );
        write(
            &work_dir.join(".theway/templates/review.md"),
            "---\nname: review\ndescription: project desc\n---\nproject body",
        );
        write(
            &base.join("templates/user-only.md"),
            "---\nname: user-only\ndescription: user only\n---\nuser body",
        );

        let templates = scan_templates(&work_dir, &base);
        assert_eq!(templates.len(), 2, "{templates:?}");

        let review = templates
            .iter()
            .find(|template| template.name == "review")
            .unwrap();
        assert_eq!(review.description, "project desc", "project layer wins");
        assert_eq!(review.content, "project body");
        assert!(
            review.file_path.contains(".theway/templates/review.md"),
            "{}",
            review.file_path
        );

        let user_only = templates
            .iter()
            .find(|template| template.name == "user-only")
            .unwrap();
        assert_eq!(user_only.content, "user body");
    }

    #[test]
    fn skips_non_md_and_does_not_recurse() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/notes.txt"),
            "---\nname: notes\ndescription: not a template\n---\nbody",
        );
        write(
            &base.join("templates/sub/nested.md"),
            "---\nname: nested\ndescription: in a subdir\n---\nbody",
        );
        // A directory whose name ends with `.md` is not a file: skipped.
        std::fs::create_dir_all(base.join("templates/dir.md")).unwrap();

        let templates = scan_templates(&work_dir, &base);
        assert!(templates.is_empty(), "{templates:?}");
    }

    #[test]
    fn missing_name_falls_back_to_file_stem() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/no-name.md"),
            "---\ndescription: no name here\n---\nbody",
        );

        let templates = scan_templates(&work_dir, &base);
        assert_eq!(templates.len(), 1, "{templates:?}");
        assert_eq!(templates[0].name, "no-name");
        assert_eq!(templates[0].description, "no name here");
        assert_eq!(templates[0].content, "body");
    }

    #[test]
    fn missing_description_stays_empty_like_daemon() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/no-desc.md"),
            "---\nname: no-desc\n---\nbody",
        );

        let templates = scan_templates(&work_dir, &base);
        assert_eq!(
            templates.len(),
            1,
            "daemon keeps templates without a description: {templates:?}"
        );
        assert_eq!(templates[0].description, "");
    }

    #[test]
    fn no_frontmatter_uses_whole_body_and_stem() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(&base.join("templates/plain.md"), "just a body\nline two\n");

        let templates = scan_templates(&work_dir, &base);
        assert_eq!(templates.len(), 1, "{templates:?}");
        assert_eq!(templates[0].name, "plain");
        assert_eq!(templates[0].description, "");
        // No frontmatter: the daemon keeps the normalized body untrimmed.
        assert_eq!(templates[0].content, "just a body\nline two\n");
    }

    #[test]
    fn malformed_frontmatter_is_skipped_like_daemon() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(
            &base.join("templates/bad-yaml.md"),
            "---\nname: [unclosed\n---\nbody",
        );

        let templates = scan_templates(&work_dir, &base);
        assert!(templates.is_empty(), "{templates:?}");
    }

    #[test]
    fn empty_frontmatter_is_skipped_instead_of_panicking() {
        let tmp = tempdir().unwrap();
        let work_dir = tmp.path().join("work");
        let base = tmp.path().join("base");

        write(&base.join("templates/empty.md"), "---\n---\nbody");

        let templates = scan_templates(&work_dir, &base);
        assert!(templates.is_empty(), "{templates:?}");
    }
}
