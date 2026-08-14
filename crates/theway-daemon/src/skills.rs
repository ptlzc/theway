//! Skills discovery for the CLI.
//!
//! Loads markdown skills from the project and user roots via `theway-core`'s
//! `harness::skills` loader. Project roots are scanned before user roots and
//! `.agents` before the other project roots; on a name collision the first
//! root in priority order wins, so a repo-local `.agents/skills` skill
//! overrides every other copy of the same name (issue #37).

use std::path::{Path, PathBuf};

use theway_core::{Skill, SkillDiagnostic, SkillSource};

#[cfg(feature = "local")]
use crate::env::native::NativeEnv;
#[cfg(feature = "local")]
use theway_core::load_skills;
use theway_transport::client::base_dir;
#[cfg(feature = "local")]
use tokio_util::sync::CancellationToken;

/// Ordered scan roots, highest priority first. Roots are consulted in this
/// order and the first loaded skill of a given name wins (see
/// [`load_all`]): project before user, `.agents` before `.theway` /
/// `.codex` / `.claude`.
pub fn skills_dirs(cwd: &Path) -> Vec<(PathBuf, SkillSource)> {
    let user = user_config_root();
    vec![
        (cwd.join(".agents").join("skills"), SkillSource::Project),
        (cwd.join(".theway").join("skills"), SkillSource::Project),
        (cwd.join(".codex").join("skills"), SkillSource::Project),
        (cwd.join(".claude").join("skills"), SkillSource::Project),
        (user.join(".agents").join("skills"), SkillSource::User),
        (user.join("skills"), SkillSource::User),
        (user.join(".codex").join("skills"), SkillSource::User),
        (user.join(".claude").join("skills"), SkillSource::User),
    ]
}

/// User-config root: `$HOME` when set, else the `~/.theway` base dir (the
/// pre-#37 user root) itself so a relocated `THEWAY_DIR` keeps working.
fn user_config_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(base_dir)
}

/// Final loaded skills plus any diagnostics from the walk. The CLI surfaces a summary line
/// from the diagnostics (count + first message) at startup if non-empty.
pub struct LoadedSkills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Load skills from every root in [`skills_dirs`], first-wins on name
/// collision (highest-priority root loaded first). Missing directories are
/// silently skipped — most users won't have all of them.
///
/// `sandbox`-only builds return empty: local skill discovery walks the OS filesystem
/// via [`NativeEnv`], which is a `local`-feature capability (daemon-kernel-layers).
#[cfg(feature = "local")]
pub async fn load_all(cwd: &Path) -> LoadedSkills {
    let env = NativeEnv::new(cwd.to_string_lossy().to_string());
    let cancel = CancellationToken::new();

    let mut combined = Vec::<Skill>::new();
    let mut diagnostics = Vec::<SkillDiagnostic>::new();

    // Load in priority order so the first copy of a name survives; the runtime
    // walker leaves `source` at its default, so tag each root's skills with
    // the project-vs-user source here, where the distinction is known.
    for (dir, source) in skills_dirs(cwd) {
        let s = dir.to_string_lossy().to_string();
        let out = load_skills(&env, &[s.as_str()], cancel.clone()).await;
        diagnostics.extend(out.diagnostics);
        for mut skill in out.skills {
            skill.source = source;
            dedupe_first_wins(&mut combined, skill);
        }
    }

    LoadedSkills {
        skills: combined,
        diagnostics,
    }
}

/// Sandbox-only stub (see the `local` impl above).
///
/// Sandbox builds must never degrade silently: skill discovery is unavailable
/// without the `local` feature, so the composition root logs an explicit warn
/// instead of looking like an empty-but-healthy skill directory. The
/// once-per-startup semantics are guaranteed by the callers (the composition
/// root loads skills once), not by this stub.
#[cfg(not(feature = "local"))]
pub async fn load_all(_cwd: &Path) -> LoadedSkills {
    tracing::warn!(
        "skill discovery unavailable in sandbox build — loading no skills (the sandbox feature \
         has no local filesystem access)"
    );
    LoadedSkills {
        skills: Vec::new(),
        diagnostics: Vec::new(),
    }
}

/// Insert `skill` into `combined` unless a same-name skill was already loaded
/// from a higher-priority root (issue #37: first-loaded wins, `.agents` has
/// the highest weight).
#[cfg(feature = "local")]
fn dedupe_first_wins(combined: &mut Vec<Skill>, skill: Skill) {
    if !combined.iter().any(|s| s.name == skill.name) {
        combined.push(skill);
    }
}
