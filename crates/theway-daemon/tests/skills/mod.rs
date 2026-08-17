//! Tests for `skills` — split out of src (see docs/rust-test-files.md).

use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

fn daemon_paths(home: &Path, base: &Path, work_dir: &Path, extras: Vec<PathBuf>) -> DaemonPaths {
    DaemonPaths {
        base: base.to_path_buf(),
        home: home.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        extra_skill_dirs: Arc::new(RwLock::new(extras)),
    }
}

/// Write `<root>/skills/<name>/SKILL.md` and return the SKILL.md path.
fn write_skill(root: &Path, name: &str, description: &str, body: &str) -> PathBuf {
    let dir = root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
    let path = dir.join("SKILL.md");
    std::fs::write(&path, content).unwrap();
    path
}

fn skill(name: &str, source: SkillSource, description: &str, file_path: &str) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: file_path.into(),
        content: "body".into(),
        disable_model_invocation: false,
        source,
    }
}

#[test]
fn skills_dirs_orders_roots_and_tags_sources() {
    // Arrange
    let paths = daemon_paths(
        Path::new("/h"),
        Path::new("/b"),
        Path::new("/w"),
        vec![PathBuf::from("/x1"), PathBuf::from("/x2")],
    );

    // Act
    let dirs: Vec<(String, SkillSource)> = skills_dirs(&paths)
        .into_iter()
        .map(|(p, s)| (p.to_string_lossy().into_owned(), s))
        .collect();

    // Assert
    assert_eq!(
        dirs,
        vec![
            ("/x1".to_string(), SkillSource::User),
            ("/x2".to_string(), SkillSource::User),
            ("/w/.agents/skills".to_string(), SkillSource::Project),
            ("/w/.theway/skills".to_string(), SkillSource::Project),
            ("/w/.codex/skills".to_string(), SkillSource::Project),
            ("/w/.claude/skills".to_string(), SkillSource::Project),
            ("/b/skills".to_string(), SkillSource::User),
            ("/h/.agents/skills".to_string(), SkillSource::User),
            ("/h/skills".to_string(), SkillSource::User),
            ("/h/.codex/skills".to_string(), SkillSource::User),
            ("/h/.claude/skills".to_string(), SkillSource::User),
        ]
    );
}

#[tokio::test]
async fn load_all_discovers_roots_tags_sources_and_first_wins() {
    // Arrange
    let home = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();

    // Same name in the strongest roots: the controller extra must win.
    let extra_skill = write_skill(extra.path(), "shared", "extra-desc", "EXTRA BODY");
    write_skill(
        &work_dir.path().join(".agents"),
        "shared",
        "agents-desc",
        "AGENTS BODY",
    );
    write_skill(
        &work_dir.path().join(".theway"),
        "project-only",
        "project-desc",
        "PROJECT BODY",
    );
    write_skill(base.path(), "base-only", "base-desc", "BASE BODY");
    write_skill(home.path(), "home-only", "home-desc", "HOME BODY");

    let paths = daemon_paths(
        home.path(),
        base.path(),
        work_dir.path(),
        vec![extra.path().to_path_buf()],
    );

    // Act
    let loaded = load_all(&paths).await;

    // Assert
    assert!(
        loaded.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        loaded.diagnostics
    );
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["shared", "project-only", "base-only", "home-only"]);

    let shared = loaded
        .skills
        .iter()
        .find(|s| s.name == "shared")
        .expect("shared skill present");
    assert_eq!(shared.description, "extra-desc");
    assert_eq!(shared.source, SkillSource::User);
    assert_eq!(shared.file_path, extra_skill.to_string_lossy().into_owned());

    let project = loaded
        .skills
        .iter()
        .find(|s| s.name == "project-only")
        .expect("project skill present");
    assert_eq!(project.source, SkillSource::Project);

    let base_skill = loaded
        .skills
        .iter()
        .find(|s| s.name == "base-only")
        .expect("base skill present");
    assert_eq!(base_skill.source, SkillSource::User);

    let home_skill = loaded
        .skills
        .iter()
        .find(|s| s.name == "home-only")
        .expect("home skill present");
    assert_eq!(home_skill.source, SkillSource::User);
}

#[tokio::test]
async fn load_all_missing_roots_loads_cleanly() {
    // Arrange
    let home = tempfile::tempdir().unwrap();
    let base = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let paths = daemon_paths(home.path(), base.path(), work_dir.path(), Vec::new());

    // Act
    let loaded = load_all(&paths).await;

    // Assert
    assert!(loaded.skills.is_empty());
    assert!(loaded.diagnostics.is_empty());
}

#[test]
fn dedupe_first_wins_skips_same_name_and_keeps_new_names() {
    // Arrange
    let mut combined = vec![skill("a", SkillSource::User, "first", "/tmp/a")];

    // Act: same name from a lower-priority root is skipped.
    dedupe_first_wins(
        &mut combined,
        skill("a", SkillSource::Project, "second", "/tmp/project-a"),
    );

    // Assert
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0].description, "first");
    assert_eq!(combined[0].source, SkillSource::User);

    // Act: a new name is appended.
    dedupe_first_wins(
        &mut combined,
        skill("b", SkillSource::Project, "second-name", "/tmp/b"),
    );

    // Assert
    assert_eq!(combined.len(), 2);
    assert_eq!(combined[1].name, "b");
    assert_eq!(combined[1].source, SkillSource::Project);
}
