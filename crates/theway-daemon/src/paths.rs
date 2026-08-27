//! Daemon path context (issue #66): every host path the daemon kernel needs is
//! resolved ONCE at the CLI boundary ([`DaemonPaths::from_cli`]) and then
//! handed to kernel modules as explicit parameters. Kernel code must not read
//! `HOME` / `THEWAY_DIR` (or any path-shaped env var) itself — the environment
//! is consulted only inside [`DaemonPaths::from_cli`].
//!
//! **Exception.** `theway_contract::config::base_dir()` (and its
//! `theway_transport::{client, config}` re-exports) stays env-driven on
//! purpose: transport port-file discovery and the inbox path are the shared
//! client↔daemon discovery contract — the TUI/CLI client derives the same
//! `<THEWAY_DIR>/daemon-port-<cwd-hash>` file from the environment to find a
//! running daemon, so that derivation must stay identical on both sides.
//! Call sites that implement that contract (transport port-file discovery,
//! inbox) are exempt from the "no env reads in the kernel" rule.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Resolved host-path context for one daemon process.
///
/// Built once at startup by the composition root (`bin/thewayd.rs`) from CLI
/// flags + environment; every consumer afterwards receives plain `Path`
/// values.
#[derive(Clone, Debug)]
pub struct DaemonPaths {
    /// The theway base dir (`config.toml`, `skill-overrides.json`, `skills/`,
    /// `extensions/`, …): `$THEWAY_DIR` when set, else `<home>/.theway`.
    pub base: PathBuf,
    /// The user home dir (user-level `.agents` / `.claude` config roots):
    /// the `--home` flag when given, else `$HOME`.
    pub home: PathBuf,
    /// The working directory (session repo + tool execution): the `--cwd`
    /// flag when given, else the process cwd. Best-effort canonicalized;
    /// a failed canonicalize keeps the original value.
    pub work_dir: PathBuf,
    /// Extra skill directories supplied via `--skills-dir` (repeatable);
    /// consumed by the skill-loading node (issue #66 follow-up). Dynamically
    /// replaceable at runtime via `SetSkillDirs` (issue #68) — shared behind
    /// an `Arc<RwLock<..>>` so every `Clone` of this struct observes the same
    /// current value; read through [`Self::current_extra_skill_dirs`] and
    /// written through [`Self::set_extra_skill_dirs`].
    pub extra_skill_dirs: Arc<RwLock<Vec<PathBuf>>>,
}

impl DaemonPaths {
    /// Resolve the daemon path context at the CLI boundary. This is the ONLY
    /// place in the daemon that reads `THEWAY_DIR` / `HOME`.
    ///
    /// Precedence:
    /// - `base`: `--theway-dir` overrides `$THEWAY_DIR`, which overrides the
    ///   `<home>/.theway` derivation.
    /// - `home`: the explicit flag overrides `$HOME`.
    /// - `work_dir`: the explicit flag overrides the process cwd; the result
    ///   is canonicalized best-effort (a failed canonicalize — e.g. the dir
    ///   does not exist yet — keeps the original value so the caller can
    ///   still surface a "cd into …" error).
    pub fn from_cli(
        cwd: Option<PathBuf>,
        home: Option<PathBuf>,
        extra_skill_dirs: Vec<PathBuf>,
    ) -> Self {
        Self::from_cli_with_base(cwd, home, extra_skill_dirs, None)
    }

    /// [`from_cli`] with an explicit base dir (`thewayd --theway-dir`).
    pub fn from_cli_with_base(
        cwd: Option<PathBuf>,
        home: Option<PathBuf>,
        extra_skill_dirs: Vec<PathBuf>,
        theway_dir: Option<PathBuf>,
    ) -> Self {
        let home = home.unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let base = theway_dir
            .or_else(|| std::env::var_os("THEWAY_DIR").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".theway"));
        let work_dir = match cwd {
            Some(dir) => dir,
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        let work_dir = work_dir.canonicalize().unwrap_or(work_dir);
        Self {
            base,
            home,
            work_dir,
            extra_skill_dirs: Arc::new(RwLock::new(extra_skill_dirs)),
        }
    }

    /// The user-global skills root: `<base>/skills`.
    pub fn skills_root(&self) -> PathBuf {
        self.base.join("skills")
    }

    /// Derive a cwd-scoped view while sharing mutable extra skill directories.
    pub fn with_work_dir(&self, work_dir: impl Into<PathBuf>) -> Self {
        let work_dir = work_dir.into();
        let work_dir = work_dir.canonicalize().unwrap_or(work_dir);
        Self {
            base: self.base.clone(),
            home: self.home.clone(),
            work_dir,
            extra_skill_dirs: self.extra_skill_dirs.clone(),
        }
    }

    /// Replace the extra skill directories at runtime (issue #68: applied by
    /// the serialized event loop when a `SetSkillDirs` command lands). The
    /// change is visible through every `Clone` of this struct.
    pub fn set_extra_skill_dirs(&self, dirs: Vec<PathBuf>) {
        *self.extra_skill_dirs.write().unwrap() = dirs;
    }

    /// Snapshot of the current extra skill directories (issue #68: the list
    /// may be replaced at runtime via [`Self::set_extra_skill_dirs`]).
    pub fn current_extra_skill_dirs(&self) -> Vec<PathBuf> {
        self.extra_skill_dirs.read().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    //! Env-mutating tests: `from_cli` is the single boundary that reads
    //! `THEWAY_DIR` / `HOME`, so these tests set/restore both. They share the
    //! crate-wide [`crate::test_env`] lock (issue #16) with every other
    //! bridged module that mutates process env, and hold the guard across the
    //! whole test body so a racing test never observes a half-swapped env.

    use super::*;
    use crate::test_env::{ENV_LOCK, EnvGuard};

    fn canonical(path: &std::path::Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    #[test]
    fn theway_dir_overrides_home_derived_base() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::set("THEWAY_DIR", "/custom/theway");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli(None, Some(PathBuf::from("/flag-home")), Vec::new());
        assert_eq!(paths.base, PathBuf::from("/custom/theway"));
        assert_eq!(paths.home, PathBuf::from("/flag-home"));
        assert_eq!(paths.skills_root(), PathBuf::from("/custom/theway/skills"));
    }

    #[test]
    fn explicit_home_overrides_env_home_and_derives_base() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli(None, Some(PathBuf::from("/flag-home")), Vec::new());
        assert_eq!(paths.home, PathBuf::from("/flag-home"));
        assert_eq!(paths.base, PathBuf::from("/flag-home/.theway"));
    }

    #[test]
    fn theway_dir_flag_overrides_env_and_home() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::set("THEWAY_DIR", "/env/theway");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli_with_base(
            None,
            Some(PathBuf::from("/flag-home")),
            Vec::new(),
            Some(PathBuf::from("/custom/theway")),
        );
        assert_eq!(paths.base, PathBuf::from("/custom/theway"));
        assert_eq!(paths.skills_root(), PathBuf::from("/custom/theway/skills"));
    }

    #[test]
    fn from_cli_with_base_keeps_env_precedence_without_flag() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::set("THEWAY_DIR", "/env/theway");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli_with_base(
            None,
            Some(PathBuf::from("/flag-home")),
            Vec::new(),
            None,
        );
        assert_eq!(paths.base, PathBuf::from("/env/theway"));
    }

    #[test]
    fn from_cli_with_base_defaults_to_home_theway() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli_with_base(
            None,
            Some(PathBuf::from("/flag-home")),
            Vec::new(),
            None,
        );
        assert_eq!(paths.base, PathBuf::from("/flag-home/.theway"));
    }

    #[test]
    fn env_home_derives_base_when_no_flag() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli(None, None, Vec::new());
        assert_eq!(paths.home, PathBuf::from("/env-home"));
        assert_eq!(paths.base, PathBuf::from("/env-home/.theway"));
    }

    #[test]
    fn work_dir_falls_back_to_process_cwd() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");

        let paths = DaemonPaths::from_cli(None, Some(PathBuf::from("/h")), Vec::new());
        let expected = std::env::current_dir().unwrap();
        assert_eq!(paths.work_dir, canonical(&expected));
    }

    #[test]
    fn explicit_work_dir_wins_and_survives_failed_canonicalize() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");

        // Existing dir: canonicalized.
        let temp = tempfile::tempdir().unwrap();
        let paths = DaemonPaths::from_cli(Some(temp.path().to_path_buf()), None, Vec::new());
        assert_eq!(paths.work_dir, canonical(temp.path()));

        // Missing dir: canonicalize fails → the original value is kept so the
        // composition root can still fail with a "cd into …" error.
        let missing = PathBuf::from("/nonexistent-theway-work-dir-66");
        let paths = DaemonPaths::from_cli(Some(missing.clone()), None, Vec::new());
        assert_eq!(paths.work_dir, missing);
    }

    #[test]
    fn extra_skill_dirs_are_carried_through() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");

        let extras = vec![PathBuf::from("/a/skills"), PathBuf::from("/b/skills")];
        let paths = DaemonPaths::from_cli(None, Some(PathBuf::from("/h")), extras.clone());
        assert_eq!(paths.current_extra_skill_dirs(), extras);
    }

    #[test]
    fn extra_skill_dirs_update_dynamically() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");

        let paths = DaemonPaths::from_cli(
            None,
            Some(PathBuf::from("/h")),
            vec![PathBuf::from("/a/skills")],
        );
        assert_eq!(
            paths.current_extra_skill_dirs(),
            vec![PathBuf::from("/a/skills")]
        );

        // Runtime replacement (issue #68 `SetSkillDirs`): the accessor sees
        // the new list, not the startup value.
        paths.set_extra_skill_dirs(vec![PathBuf::from("/x/skills"), PathBuf::from("/y/skills")]);
        assert_eq!(
            paths.current_extra_skill_dirs(),
            vec![PathBuf::from("/x/skills"), PathBuf::from("/y/skills")]
        );

        // Clearing is a legitimate update too (empty list → no extras).
        paths.set_extra_skill_dirs(Vec::new());
        assert!(paths.current_extra_skill_dirs().is_empty());
    }

    #[test]
    fn with_work_dir_preserves_shared_base_home_and_extra_skill_dirs() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");
        let _home_env = EnvGuard::set("HOME", "/env-home");

        let paths = DaemonPaths::from_cli(
            None,
            Some(PathBuf::from("/flag-home")),
            vec![PathBuf::from("/shared/skills")],
        );
        let other = tempfile::tempdir().unwrap();
        let derived = paths.with_work_dir(other.path());

        assert_eq!(derived.base, paths.base);
        assert_eq!(derived.home, paths.home);
        assert_eq!(derived.work_dir, canonical(other.path()));
        assert!(Arc::ptr_eq(
            &derived.extra_skill_dirs,
            &paths.extra_skill_dirs
        ));

        paths.set_extra_skill_dirs(vec![PathBuf::from("/updated/skills")]);
        assert_eq!(
            derived.current_extra_skill_dirs(),
            vec![PathBuf::from("/updated/skills")]
        );
    }

    #[test]
    fn extra_skill_dirs_shared_across_clones() {
        let _serial = ENV_LOCK.lock().unwrap();
        let _theway = EnvGuard::remove("THEWAY_DIR");

        let paths = DaemonPaths::from_cli(None, Some(PathBuf::from("/h")), Vec::new());
        let cloned = paths.clone();

        // The backing list is shared behind an Arc: an update through one
        // handle is observed through the other (issue #68 — the event loop
        // and the skill loader may hold separate clones of the same context).
        paths.set_extra_skill_dirs(vec![PathBuf::from("/shared/skills")]);
        assert_eq!(
            cloned.current_extra_skill_dirs(),
            vec![PathBuf::from("/shared/skills")]
        );
        cloned.set_extra_skill_dirs(Vec::new());
        assert!(paths.current_extra_skill_dirs().is_empty());
    }
}
