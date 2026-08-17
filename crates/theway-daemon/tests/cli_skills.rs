//! End-to-end test for the daemon's skills loader wiring.
//!
//! Strategy: simulate the root layout — controller extras (`--skills-dir`), project roots
//! under the work dir, the native install target `<base>/skills`, and the `$HOME` roots —
//! using tempdirs for home/base/work-dir. Then run the same loader order the daemon runs
//! and assert:
//!   1. Skills from every tier are loaded and tagged with the right source.
//!   2. First-wins on a shared name, in priority order: extras > project > `<base>/skills`
//!      (install target) > home roots (issues #37 + #66).
//!   3. Loaded skills are stitched into the final harness system prompt.
//!
//! This exercises the public surface only — no direct calls into the harness-internal walker.
//! If the daemon ever changes how it picks the roots, this test catches it.
//!
//! Local-only suite: the loader mirror walks the FS through `NativeEnv`, which is
//! compiled out of sandbox-only builds (issue #64).
#![cfg(feature = "local")]

use std::path::{Path, PathBuf};

use tempfile::TempDir;
use theway_core::{
    AgentHarness, AgentHarnessOptions, MemorySessionStorage, Session, ThinkingLevel,
};
use theway_daemon::DaemonPaths;

/// Mirror of the daemon's `skills` module loader. The ordering is duplicated verbatim from
/// `theway_daemon::skills::skills_dirs` so the suite also catches accidental drift between
/// the mirror and the real function (a direct ordering assertion on the real function lives
/// in [`real_skills_dirs_order_matches_contract`]).
mod skills_mirror {
    use std::path::PathBuf;

    use theway_core::{Skill, SkillDiagnostic, SkillSource, load_skills};
    use theway_daemon::DaemonPaths;
    use theway_daemon::env::native::NativeEnv;
    use tokio_util::sync::CancellationToken;

    pub struct LoadedSkills {
        pub skills: Vec<Skill>,
        pub diagnostics: Vec<SkillDiagnostic>,
    }

    pub async fn load_all(paths: &DaemonPaths) -> LoadedSkills {
        let env = NativeEnv::new(paths.work_dir.to_string_lossy().to_string());
        let cancel = CancellationToken::new();
        let mut combined = Vec::<Skill>::new();
        let mut diagnostics = Vec::<SkillDiagnostic>::new();
        // Mirror the real `skills::load_all` root order (issue #66): controller
        // extras (`--skills-dir`) first, then the project roots under the work
        // dir (`.agents` before the other project roots), then `<base>/skills`
        // (the install target), then the home roots; the first copy of each
        // name wins (issue #37).
        let mut roots: Vec<(PathBuf, SkillSource)> = Vec::new();
        // Snapshot the dynamically updatable extras once (issue #68).
        let extras = paths.current_extra_skill_dirs();
        for extra in &extras {
            roots.push((extra.clone(), SkillSource::User));
        }
        roots.push((
            paths.work_dir.join(".agents").join("skills"),
            SkillSource::Project,
        ));
        roots.push((
            paths.work_dir.join(".theway").join("skills"),
            SkillSource::Project,
        ));
        roots.push((
            paths.work_dir.join(".codex").join("skills"),
            SkillSource::Project,
        ));
        roots.push((
            paths.work_dir.join(".claude").join("skills"),
            SkillSource::Project,
        ));
        roots.push((paths.skills_root(), SkillSource::User));
        roots.push((paths.home.join(".agents").join("skills"), SkillSource::User));
        roots.push((paths.home.join("skills"), SkillSource::User));
        roots.push((paths.home.join(".codex").join("skills"), SkillSource::User));
        roots.push((paths.home.join(".claude").join("skills"), SkillSource::User));
        for (dir, source) in roots {
            let s = dir.to_string_lossy().to_string();
            let out = load_skills(&env, &[s.as_str()], cancel.clone()).await;
            diagnostics.extend(out.diagnostics);
            for mut skill in out.skills {
                skill.source = source;
                if !combined.iter().any(|s| s.name == skill.name) {
                    combined.push(skill);
                }
            }
        }
        LoadedSkills {
            skills: combined,
            diagnostics,
        }
    }
}

/// Build a plain [`DaemonPaths`] without touching the env (`from_cli` reads
/// `HOME` / `THEWAY_DIR`; tests supply every root explicitly).
fn daemon_paths(home: &Path, base: &Path, work_dir: &Path, extras: Vec<PathBuf>) -> DaemonPaths {
    DaemonPaths {
        base: base.to_path_buf(),
        home: home.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        extra_skill_dirs: std::sync::Arc::new(std::sync::RwLock::new(extras)),
    }
}

/// Write `<root>/skills/<name>/SKILL.md`.
fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n");
    std::fs::write(dir.join("SKILL.md"), content).unwrap();
}

fn faux_model() -> theway_llm_provider::Model {
    theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        headers: None,
        compat: None,
    }
}

#[tokio::test]
async fn project_skill_overrides_user_skill_with_same_name() {
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // user-global skill (home root `<home>/skills`)
    write_skill(home.path(), "shared", "user-version", "USER BODY");
    // project-local skill with same name — should win
    write_skill(
        &cwd.path().join(".theway"),
        "shared",
        "project-version",
        "PROJECT BODY",
    );
    // user-only skill (no project counterpart)
    write_skill(home.path(), "only-user", "user-only", "ONLY USER BODY");

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    assert!(
        loaded.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        loaded.diagnostics
    );
    let names: Vec<&str> = loaded.skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"shared"));
    assert!(names.contains(&"only-user"));
    let shared = loaded.skills.iter().find(|s| s.name == "shared").unwrap();
    assert_eq!(
        shared.description, "project-version",
        "project should override user on same name"
    );
    assert!(
        shared.content.contains("PROJECT BODY"),
        "shared content should come from project: {:?}",
        shared.content
    );

    // Now feed into an actual harness and confirm the system prompt includes both skills.
    let storage = std::sync::Arc::new(MemorySessionStorage::new());
    let session = Session::new(storage as std::sync::Arc<dyn theway_core::SessionStorage>);
    let mut opts = AgentHarnessOptions::new(faux_model(), session);
    opts.system_prompt = "base prompt".into();
    opts.thinking_level = ThinkingLevel::Off;
    opts.skills = loaded.skills.clone();
    let harness = AgentHarness::new(opts);

    let prompt = harness.system_prompt();
    assert!(prompt.contains("base prompt"));
    assert!(
        prompt.contains("name: shared"),
        "system prompt should list 'shared' skill: {prompt}"
    );
    assert!(
        prompt.contains("name: only-user"),
        "system prompt should list 'only-user' skill: {prompt}"
    );
    // Description identifies which version landed. Skill bodies are invoked via the `skill`
    // tool, not inlined into the prompt — so we don't assert on `PROJECT BODY` here.
    assert!(
        prompt.contains("description: project-version"),
        "project version of 'shared' should win in system prompt: {prompt}"
    );
    assert!(
        !prompt.contains("description: user-version"),
        "user version of 'shared' must NOT appear in the listing: {prompt}"
    );

    // Sanity-check: the project body actually lives on the in-memory skill record (so when the
    // model later invokes `skill('shared')`, it gets the project copy).
    let kept = harness
        .skills()
        .into_iter()
        .find(|s| s.name == "shared")
        .expect("shared skill present");
    assert!(
        kept.content.contains("PROJECT BODY"),
        "harness should keep project body for the shared skill: {:?}",
        kept.content
    );
}

#[tokio::test]
async fn missing_roots_load_cleanly() {
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    assert!(loaded.skills.is_empty());
    assert!(
        loaded.diagnostics.is_empty(),
        "non-existent roots should produce no diagnostics: {:#?}",
        loaded.diagnostics
    );
}

#[tokio::test]
async fn loader_tags_skill_source_per_root() {
    use theway_core::SkillSource;
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // One skill in each root, distinct names so no shadowing.
    write_skill(home.path(), "user-skill", "u", "USER");
    write_skill(&cwd.path().join(".theway"), "project-skill", "p", "PROJECT");

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;

    let user = loaded
        .skills
        .iter()
        .find(|s| s.name == "user-skill")
        .expect("user skill loaded");
    let project = loaded
        .skills
        .iter()
        .find(|s| s.name == "project-skill")
        .expect("project skill loaded");

    assert_eq!(
        user.source,
        SkillSource::User,
        "skill from the $HOME skill roots must be tagged User"
    );
    assert_eq!(
        project.source,
        SkillSource::Project,
        "skill from <cwd>/.theway/skills must be tagged Project"
    );
    // The display label the `/skills` listing renders comes straight off the field now.
    assert_eq!(user.source.label(), "user");
    assert_eq!(project.source.label(), "project");
}

#[tokio::test]
async fn loader_tags_project_source_when_project_shadows_user() {
    use theway_core::SkillSource;
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // Same name in both roots — project wins, and the surviving entry must carry the
    // Project source (not the User source it would have had if shadowing dropped the tag).
    write_skill(home.path(), "shared", "user-version", "USER BODY");
    write_skill(
        &cwd.path().join(".theway"),
        "shared",
        "project-version",
        "PROJECT BODY",
    );

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    let shared = loaded.skills.iter().find(|s| s.name == "shared").unwrap();
    assert_eq!(
        shared.source,
        SkillSource::Project,
        "project-shadowed skill must report Project source"
    );
}

#[tokio::test]
async fn agents_skills_have_highest_weight_first_wins() {
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // Same name across .agents / .codex / .claude: the .agents copy is
    // loaded first and wins; no duplicate is kept (issue #37).
    write_skill(
        &cwd.path().join(".agents"),
        "shared",
        "agents-version",
        "AGENTS BODY",
    );
    write_skill(
        &cwd.path().join(".codex"),
        "shared",
        "codex-version",
        "CODEX BODY",
    );
    write_skill(
        &cwd.path().join(".claude"),
        "shared",
        "claude-version",
        "CLAUDE BODY",
    );

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    assert!(
        loaded.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        loaded.diagnostics
    );
    let shared: Vec<&theway_core::Skill> = loaded
        .skills
        .iter()
        .filter(|s| s.name == "shared")
        .collect();
    assert_eq!(shared.len(), 1, "only the first copy may be kept");
    assert_eq!(
        shared[0].description, "agents-version",
        ".agents has the highest weight among the project roots and must win"
    );
    assert!(shared[0].content.contains("AGENTS BODY"));
}

#[tokio::test]
async fn base_skills_are_discovered_and_tagged_user() {
    use theway_core::SkillSource;
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // The native install target: `<base>/skills/<name>/SKILL.md`. Issue #66: this root was
    // never scanned before, so an installed skill never showed up — now it must.
    write_skill(base.path(), "installed", "installed-desc", "INSTALLED BODY");

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    assert!(
        loaded.diagnostics.is_empty(),
        "unexpected diagnostics: {:#?}",
        loaded.diagnostics
    );
    let installed = loaded
        .skills
        .iter()
        .find(|s| s.name == "installed")
        .expect("skill under <base>/skills (install target) must be discovered");
    assert_eq!(
        installed.source,
        SkillSource::User,
        "skill from <base>/skills must be tagged User"
    );
    assert!(installed.content.contains("INSTALLED BODY"));
}

#[tokio::test]
async fn extra_dir_skill_beats_project_root_same_name() {
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let extra = TempDir::new().unwrap();

    // An extra `--skills-dir` root is a skills directory itself: drop
    // `<name>/SKILL.md` directly inside it.
    let extra_root = extra.path().join("skills");
    write_skill(extra.path(), "shared", "extra-version", "EXTRA BODY");
    // Same name under the two strongest project roots.
    write_skill(
        &cwd.path().join(".agents"),
        "shared",
        "agents-version",
        "AGENTS BODY",
    );
    write_skill(
        &cwd.path().join(".theway"),
        "shared",
        "project-version",
        "PROJECT BODY",
    );

    let paths = daemon_paths(
        home.path(),
        base.path(),
        cwd.path(),
        vec![extra_root.clone()],
    );
    let loaded = skills_mirror::load_all(&paths).await;
    let shared: Vec<&theway_core::Skill> = loaded
        .skills
        .iter()
        .filter(|s| s.name == "shared")
        .collect();
    assert_eq!(shared.len(), 1, "only the first copy may be kept");
    assert_eq!(
        shared[0].description, "extra-version",
        "--skills-dir extras are controller-explicit and must beat project roots"
    );
    assert!(shared[0].content.contains("EXTRA BODY"));
    // The surviving copy comes from the extra root, not a project root.
    assert!(
        shared[0]
            .file_path
            .starts_with(&extra_root.to_string_lossy().into_owned()),
        "winning skill must come from the extra dir: {}",
        shared[0].file_path
    );
}

#[tokio::test]
async fn base_skills_beat_home_skills_same_name() {
    let home = TempDir::new().unwrap();
    let base = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();

    // Same name in the install target `<base>/skills` and the `<home>/skills` root:
    // base is scanned first among the user roots and must win (issue #66).
    write_skill(base.path(), "shared", "base-version", "BASE BODY");
    write_skill(home.path(), "shared", "home-version", "HOME BODY");

    let paths = daemon_paths(home.path(), base.path(), cwd.path(), Vec::new());
    let loaded = skills_mirror::load_all(&paths).await;
    let shared: Vec<&theway_core::Skill> = loaded
        .skills
        .iter()
        .filter(|s| s.name == "shared")
        .collect();
    assert_eq!(shared.len(), 1, "only the first copy may be kept");
    assert_eq!(
        shared[0].description, "base-version",
        "<base>/skills (install target) is scanned before the home roots and must win"
    );
    assert!(shared[0].content.contains("BASE BODY"));
}

#[test]
fn real_skills_dirs_order_matches_contract() {
    use theway_core::SkillSource;
    use theway_daemon::skills::skills_dirs;

    // Assert the REAL function's ordering (not the mirror's): extras first, then the
    // project roots in #37 order, then `<base>/skills` ahead of the home roots (issue #66).
    let paths = daemon_paths(
        Path::new("/h"),
        Path::new("/b"),
        Path::new("/w"),
        vec![PathBuf::from("/x1"), PathBuf::from("/x2")],
    );
    let dirs: Vec<(String, SkillSource)> = skills_dirs(&paths)
        .into_iter()
        .map(|(p, s)| (p.to_string_lossy().into_owned(), s))
        .collect();
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
