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
    /// consumed by the skill-loading node (issue #66 follow-up).
    pub extra_skill_dirs: Vec<PathBuf>,
}

impl DaemonPaths {
    /// Resolve the daemon path context at the CLI boundary. This is the ONLY
    /// place in the daemon that reads `THEWAY_DIR` / `HOME`.
    ///
    /// Precedence:
    /// - `base`: `$THEWAY_DIR` overrides the `<home>/.theway` derivation.
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
        let home = home.unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
        let base = std::env::var_os("THEWAY_DIR")
            .map(PathBuf::from)
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
            extra_skill_dirs,
        }
    }

    /// The user-global skills root: `<base>/skills`.
    pub fn skills_root(&self) -> PathBuf {
        self.base.join("skills")
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
        assert_eq!(paths.extra_skill_dirs, extras);
    }
}
