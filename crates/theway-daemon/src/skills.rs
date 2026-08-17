//! Skills discovery for the daemon.
//!
//! Loads markdown skills from the controller-supplied extras, project, and
//! user roots via `theway-core`'s `harness::skills` loader. Roots are scanned
//! in priority order and on a name collision the FIRST loaded copy wins
//! (issue #37 first-wins semantics), highest priority first:
//!
//! 1. `--skills-dir` extras ([`DaemonPaths::extra_skill_dirs`]) — explicitly
//!    supplied by the controller, so they beat everything else.
//! 2. Project roots under the work dir: `.agents` / `.theway` / `.codex` /
//!    `.claude` skills (order unchanged from #37).
//! 3. `<base>/skills` — the native theway install target. Now part of the
//!    scan (issue #66: fixes the #37 leftover where the install target was
//!    never scanned), and consulted before the other user roots.
//! 4. Home roots: `.agents/skills`, `skills`, `.codex/skills`,
//!    `.claude/skills` (order unchanged).
//!
//! The scan list and the install target agree: `install_skill` writes into
//! `<base>/skills`, which is exactly root 3, so an installed skill is picked
//! up by the next load/reload without any extra wiring (issue #66).

use std::path::PathBuf;

use theway_core::{Skill, SkillDiagnostic, SkillSource};

#[cfg(feature = "local")]
use crate::env::native::NativeEnv;
use crate::paths::DaemonPaths;
#[cfg(feature = "local")]
use theway_core::load_skills;
#[cfg(feature = "local")]
use tokio_util::sync::CancellationToken;

/// Ordered scan roots, highest priority first. Roots are consulted in this
/// order and the first loaded skill of a given name wins (see [`load_all`]):
/// controller-supplied `--skills-dir` extras, then the project roots under
/// the work dir (`.agents` before `.theway` / `.codex` / `.claude`), then
/// `<base>/skills` (the native install target), then the home roots — so an
/// installed skill shadows a same-name copy in any other user root
/// (issue #66, fixing the #37 leftover where `<base>/skills` was never
/// scanned).
///
/// Extras carry [`SkillSource::User`]: the enum has no dedicated Extra
/// variant and controller-supplied dirs are user-level additions, not
/// project-local roots — the source tag is for administration/observability,
/// while precedence comes purely from the ordering here.
pub fn skills_dirs(paths: &DaemonPaths) -> Vec<(PathBuf, SkillSource)> {
    let mut dirs: Vec<(PathBuf, SkillSource)> =
        Vec::with_capacity(paths.extra_skill_dirs.len() + 9);
    // a) Controller-supplied extras (`--skills-dir`): explicit wins over
    //    everything else.
    for extra in &paths.extra_skill_dirs {
        dirs.push((extra.clone(), SkillSource::User));
    }
    // b) Project roots under the work dir (issue #37 order unchanged).
    dirs.push((
        paths.work_dir.join(".agents").join("skills"),
        SkillSource::Project,
    ));
    dirs.push((
        paths.work_dir.join(".theway").join("skills"),
        SkillSource::Project,
    ));
    dirs.push((
        paths.work_dir.join(".codex").join("skills"),
        SkillSource::Project,
    ));
    dirs.push((
        paths.work_dir.join(".claude").join("skills"),
        SkillSource::Project,
    ));
    // c) The native install target `<base>/skills` (issue #66): scanned
    //    before the other user roots so an installed skill beats a same-name
    //    copy under the home roots.
    dirs.push((paths.skills_root(), SkillSource::User));
    // d) Home roots (order unchanged).
    dirs.push((paths.home.join(".agents").join("skills"), SkillSource::User));
    dirs.push((paths.home.join("skills"), SkillSource::User));
    dirs.push((paths.home.join(".codex").join("skills"), SkillSource::User));
    dirs.push((paths.home.join(".claude").join("skills"), SkillSource::User));
    dirs
}

/// Final loaded skills plus any diagnostics from the walk. The CLI surfaces a summary line
/// from the diagnostics (count + first message) at startup if non-empty.
pub struct LoadedSkills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Load skills from every root in [`skills_dirs`], first-wins on name
/// collision (highest-priority root loaded first): `--skills-dir` extras >
/// project roots > `<base>/skills` (the install target) > home roots. Missing
/// directories are silently skipped — most users won't have all of them.
///
/// The ordering doubles as the install-contract guarantee: `install_skill`
/// writes into `<base>/skills`, which is on the scan list, so installed
/// skills are discovered by the next load/reload — the scan roots and the
/// install target agree (issue #66).
///
/// `sandbox`-only builds return empty: local skill discovery walks the OS filesystem
/// via [`NativeEnv`], which is a `local`-feature capability (daemon-kernel-layers).
#[cfg(feature = "local")]
pub async fn load_all(paths: &DaemonPaths) -> LoadedSkills {
    let env = NativeEnv::new(paths.work_dir.to_string_lossy().to_string());
    let cancel = CancellationToken::new();

    let mut combined = Vec::<Skill>::new();
    let mut diagnostics = Vec::<SkillDiagnostic>::new();

    // Load in priority order so the first copy of a name survives; the runtime
    // walker leaves `source` at its default, so tag each root's skills with
    // the project-vs-user source here, where the distinction is known.
    for (dir, source) in skills_dirs(paths) {
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
pub async fn load_all(_paths: &DaemonPaths) -> LoadedSkills {
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
/// from a higher-priority root (issue #37: first-loaded wins; `--skills-dir`
/// extras carry the highest weight, then `.agents` among the project roots).
#[cfg(feature = "local")]
fn dedupe_first_wins(combined: &mut Vec<Skill>, skill: Skill) {
    if !combined.iter().any(|s| s.name == skill.name) {
        combined.push(skill);
    }
}
