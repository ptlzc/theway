//! Controller-side skill discovery (issue #95): the TUI owns local skill
//! scanning and provisions the daemon with the scanned catalog through the
//! settings surface (`WireDaemonConfig.skills`), so a controller-provisioned
//! daemon never reads skill files itself.
//!
//! Root order and first-wins semantics mirror the daemon's
//! `crate::skills::skills_dirs` ordering; the walk rules mirror
//! `theway_core::agent::skills` (a directory containing `SKILL.md` IS the
//! skill, directories without one are recursed, root-level `*.md` files are
//! skills, dotfiles and `node_modules` are skipped).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use theway_transport::wire::WireProvisionedSkill;

/// Frontmatter shape parsed off the `SKILL.md` head — mirrors
/// `theway_core::SkillFrontmatter` (both spellings of the disable flag).
#[derive(Clone, Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(
        default,
        rename = "disable_model_invocation",
        alias = "disable-model-invocation"
    )]
    disable_model_invocation: bool,
}

/// Scan the local skill roots and return the first-wins catalog.
///
/// Roots, in precedence order: `extras` (`--skills-dir`), project roots under
/// `cwd`, `<base>/skills`, then the home roots. `home` defaults to `$HOME`;
/// an empty home just skips those roots.
pub(crate) fn scan_skills(
    cwd: &Path,
    base: &Path,
    home: &Path,
    extras: &[PathBuf],
) -> Vec<WireProvisionedSkill> {
    let mut roots: Vec<(PathBuf, &str)> = Vec::new();
    for extra in extras {
        roots.push((extra.clone(), "user"));
    }
    for dir in ["agents", "theway", "codex", "claude"] {
        roots.push((cwd.join(format!(".{dir}")).join("skills"), "project"));
    }
    roots.push((base.join("skills"), "user"));
    if !home.as_os_str().is_empty() {
        roots.push((home.join(".agents").join("skills"), "user"));
        roots.push((home.join("skills"), "user"));
        roots.push((home.join(".codex").join("skills"), "user"));
        roots.push((home.join(".claude").join("skills"), "user"));
    }

    let mut catalog: Vec<WireProvisionedSkill> = Vec::new();
    for (root, source) in roots {
        walk_dir(&root, source, true, &mut catalog);
    }
    catalog
}

/// Mirror the core walker: if `dir` itself contains a `SKILL.md` file, that
/// file IS the skill for the dir; otherwise recurse into subdirectories and,
/// only at the initial roots, load sibling `*.md` files as skills.
fn walk_dir(dir: &Path, source: &str, root_files: bool, catalog: &mut Vec<WireProvisionedSkill>) {
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() && !is_ignored(dir) {
        if let Some(skill) = load_skill_file(&skill_md, source) {
            push_first_wins(catalog, skill);
        }
        return; // the dir is the skill; do not recurse
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // missing root is the documented default
    };
    let mut sorted: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    sorted.sort();
    for entry in &sorted {
        if is_ignored(entry) {
            continue;
        }
        if entry.is_dir() {
            walk_dir(entry, source, false, catalog);
        } else if root_files
            && entry
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            if let Some(skill) = load_skill_file(entry, source) {
                push_first_wins(catalog, skill);
            }
        }
    }
}

fn is_ignored(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    name.starts_with('.') || name == "node_modules"
}

fn load_skill_file(file_path: &Path, source: &str) -> Option<WireProvisionedSkill> {
    let raw = std::fs::read_to_string(file_path).ok()?;
    let (frontmatter, body) = parse_frontmatter(&raw)?;
    let default_name = file_path
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = frontmatter.name.unwrap_or(default_name);
    let description = frontmatter.description.unwrap_or_default();
    if name.trim().is_empty() || description.trim().is_empty() {
        return None; // mirrors core: an empty description is not a skill
    }
    Some(WireProvisionedSkill {
        name,
        description,
        content: body,
        file_path: file_path.to_string_lossy().into_owned(),
        source: source.to_string(),
        disable_model_invocation: frontmatter.disable_model_invocation,
    })
}

fn push_first_wins(catalog: &mut Vec<WireProvisionedSkill>, skill: WireProvisionedSkill) {
    if !catalog.iter().any(|existing| existing.name == skill.name) {
        catalog.push(skill);
    }
}

/// Parse the YAML frontmatter head; a missing or malformed header degrades to
/// defaults with the whole body as content (same posture as the core walker).
fn parse_frontmatter(content: &str) -> Option<(SkillFrontmatter, String)> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Some((SkillFrontmatter::default(), normalized));
    }
    let end = normalized[3..].find("\n---")? + 3;
    let yaml_str = &normalized[4..end];
    let body = normalized[end + 4..].trim().to_string();
    let frontmatter = serde_yaml::from_str(yaml_str).ok()?;
    Some((frontmatter, body))
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
    fn scans_skill_dirs_and_root_files_with_first_wins_precedence() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let base = tmp.path().join("base");
        let home = tmp.path().join("home");

        // Project layer: one SKILL.md-dir skill.
        write(
            &cwd.join(".agents/skills/release/SKILL.md"),
            "---\nname: release\ndescription: project release checklist\n---\nbody-a",
        );
        // Home layer: same-name skill (must lose).
        write(
            &home.join(".agents/skills/release/SKILL.md"),
            "---\nname: release\ndescription: home release checklist\n---\nbody-b",
        );
        // A root-level md skill in the plain home root.
        write(
            &home.join("skills/notes.md"),
            "---\nname: notes\ndescription: scratch notes\n---\nbody-c",
        );
        // base/skills layer: another skill.
        write(
            &base.join("skills/base-skill/SKILL.md"),
            "---\ndescription: base skill\n---\nbody-d",
        );

        let skills = scan_skills(&cwd, &base, &home, &[]);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"release"), "{names:?}");
        assert!(names.contains(&"notes"), "{names:?}");
        assert!(names.contains(&"base-skill"), "{names:?}");

        let release = skills.iter().find(|s| s.name == "release").unwrap();
        assert_eq!(release.content, "body-a", "project layer wins");
        assert_eq!(release.source, "project");
        assert!(
            release
                .file_path
                .ends_with(".agents/skills/release/SKILL.md")
        );
        let notes = skills.iter().find(|s| s.name == "notes").unwrap();
        assert_eq!(notes.content, "body-c", "root md files load");
        assert_eq!(notes.source, "user");
    }

    #[test]
    fn disable_flag_parses_both_spellings_and_body_drops_frontmatter() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().join("home");
        write(
            &home.join("skills/off-skill/SKILL.md"),
            "---\nname: off-skill\ndescription: disabled skill\ndisable_model_invocation: true\n---\nbody",
        );
        write(
            &home.join("skills/kebab-skill/SKILL.md"),
            "---\nname: kebab-skill\ndescription: kebab flag\ndisable-model-invocation: true\n---\nbody2",
        );
        let skills = scan_skills(
            &tmp.path().join("nope"),
            &tmp.path().join("base"),
            &home,
            &[],
        );
        let off = skills.iter().find(|s| s.name == "off-skill").unwrap();
        assert!(off.disable_model_invocation);
        let kebab = skills.iter().find(|s| s.name == "kebab-skill").unwrap();
        assert!(kebab.disable_model_invocation, "kebab alias parses");
        assert!(!off.content.contains("---"), "body must drop frontmatter");
    }

    #[test]
    fn missing_roots_and_ignored_entries_degrade_quietly() {
        let tmp = tempdir().unwrap();
        let home = tmp.path().join("home");
        write(
            &home.join("skills/node_modules/x/SKILL.md"),
            "---\nname: x\ndescription: x\n---\nx",
        );
        write(
            &home.join("skills/.hidden/SKILL.md"),
            "---\nname: hidden\ndescription: h\n---\nh",
        );
        write(
            &home.join("skills/no-desc/SKILL.md"),
            "---\nname: no-desc\n---\nbody",
        );
        let skills = scan_skills(
            &tmp.path().join("missing"),
            &tmp.path().join("missing"),
            &home,
            &[],
        );
        assert!(
            skills.is_empty(),
            "ignored/invalid entries load nothing: {skills:?}"
        );
    }
}
