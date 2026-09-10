//! Host shell resolution: which program runs a local command line, and which prefix arguments carry it.
//!
//! Resolution order:
//!
//! 1. A non-empty `THEWAY_SHELL` names the program and overrides every platform default. Its prefix arguments come from a non-empty `THEWAY_SHELL_ARGS`, split on ASCII whitespace; otherwise they are inferred from the program's file stem, matched case-insensitively: `pwsh` and `powershell` → `-NoLogo -NoProfile -Command`, `cmd` → `/C`, any other program → `-c`.
//! 2. Without an override, a Windows host takes the first `pwsh` on `PATH`, then `powershell`, then `cmd`, running PowerShell with `-NoLogo -NoProfile -Command` and `cmd` with `/C`. When none of the three is present it falls back to a non-empty `%COMSPEC%`, and finally to `cmd.exe`, both with `/C`.
//! 3. Without an override, a Unix host runs `sh -c`.
//!
//! Resolution is infallible: an unset, empty, or unusable environment yields the platform fallback rather than an error, and [`shell`] resolves once per process and caches the result.
//!
//! It lives in this leaf crate because the daemon execution paths and the local TUI controller run commands on the same host, and `theway-contract` is the only crate both depend on.
//!
//! The platform, the environment lookup, and the executable-existence check are parameters of `resolve`, so the Windows decision table runs in unit tests on any host. An executable must be an existing regular file, empty `PATH` entries are skipped, and `PATHEXT` extensions are probed in the order the variable lists them.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::OnceLock;

/// Environment variable naming the shell program; a non-empty value overrides every
/// platform default.
const SHELL_ENV: &str = "THEWAY_SHELL";

/// Environment variable supplying whitespace-separated prefix arguments for the program
/// named by `SHELL_ENV`.
const SHELL_ARGS_ENV: &str = "THEWAY_SHELL_ARGS";

/// Environment variable listing the directories probed for an executable.
const PATH_ENV: &str = "PATH";

/// Environment variable listing the executable extensions probed on a Windows host.
const PATHEXT_ENV: &str = "PATHEXT";

/// Environment variable naming the Windows command interpreter used as the last fallback.
const COMSPEC_ENV: &str = "COMSPEC";

/// Prefix arguments that make PowerShell run one command string.
const POWERSHELL_ARGS: [&str; 3] = ["-NoLogo", "-NoProfile", "-Command"];

/// Prefix argument that makes `cmd` run one command string.
const CMD_ARGS: [&str; 1] = ["/C"];

/// Prefix argument for any other program: the POSIX `-c` form.
const POSIX_ARGS: [&str; 1] = ["-c"];

/// Program names probed on `PATH` on a Windows host, in priority order.
const WINDOWS_CANDIDATES: [&str; 3] = ["pwsh", "powershell", "cmd"];

/// Extensions probed on a Windows host when `PATHEXT` is unset or empty.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Last-resort Windows command interpreter when no candidate is on `PATH` and `COMSPEC` is
/// unset or empty.
const CMD_EXE: &str = "cmd.exe";

/// Program plus prefix arguments that run one command line on the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSpec {
    /// Shell program, for example `sh`, `pwsh`, or the value of `%COMSPEC%`.
    pub program: OsString,
    /// Prefix arguments placed before the command string.
    pub args: Vec<OsString>,
}

impl ShellSpec {
    /// The prefix arguments followed by `command`, ready to pass to `Command::args`.
    pub fn command_args(&self, command: &str) -> Vec<OsString> {
        let mut args = Vec::with_capacity(self.args.len() + 1);
        args.extend(self.args.iter().cloned());
        args.push(OsString::from(command));
        args
    }

    /// Space-joined program and prefix arguments, for example `sh -c` or
    /// `pwsh -NoLogo -NoProfile -Command`.
    pub fn display(&self) -> String {
        let mut rendered = self.program.to_string_lossy().into_owned();
        for arg in &self.args {
            rendered.push(' ');
            rendered.push_str(&arg.to_string_lossy());
        }
        rendered
    }
}

/// Shell spec for the host of the running process, resolved on first call and cached for
/// every later call.
pub fn shell() -> &'static ShellSpec {
    SHELL.get_or_init(|| resolve(Platform::host(), &host_env, &host_path_exists))
}

/// Resolved host shell of this process; [`shell`] is the only reader.
static SHELL: OnceLock<ShellSpec> = OnceLock::new();

/// Reads one process environment variable.
fn host_env(key: &str) -> Option<OsString> {
    std::env::var_os(key)
}

/// Reports whether a candidate path is an existing regular file.
fn host_path_exists(candidate: &str) -> bool {
    Path::new(candidate).is_file()
}

/// Host platform a resolution targets.
///
/// It is a parameter of `resolve` rather than a direct `cfg!` read, so the Windows decision
/// table runs in unit tests on a Unix host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    /// `cfg!(windows) == false`.
    Unix,
    /// `cfg!(windows) == true`.
    Windows,
}

impl Platform {
    /// Platform of the running process.
    fn host() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }

    /// Character separating `PATH` entries.
    fn path_separator(self) -> char {
        match self {
            Self::Unix => ':',
            Self::Windows => ';',
        }
    }

    /// Character joining a `PATH` entry with a program name.
    fn dir_separator(self) -> char {
        match self {
            Self::Unix => '/',
            Self::Windows => '\\',
        }
    }
}

/// Builds the shell spec for `platform`, reading every environment value through `env` and
/// asking `exists` whether a candidate executable is present.
fn resolve(
    platform: Platform,
    env: &dyn Fn(&str) -> Option<OsString>,
    exists: &dyn Fn(&str) -> bool,
) -> ShellSpec {
    if let Some(program) = non_empty(env, SHELL_ENV) {
        let args = match non_empty(env, SHELL_ARGS_ENV) {
            Some(raw) => split_prefix_args(&raw),
            None => inferred_prefix_args(&program),
        };
        return ShellSpec { program, args };
    }
    match platform {
        Platform::Unix => make_spec("sh", &POSIX_ARGS),
        Platform::Windows => windows_default(env, exists),
    }
}

/// Windows shell without an override: the first candidate on `PATH`, then `%COMSPEC%`, then
/// `cmd.exe`.
fn windows_default(
    env: &dyn Fn(&str) -> Option<OsString>,
    exists: &dyn Fn(&str) -> bool,
) -> ShellSpec {
    for name in WINDOWS_CANDIDATES {
        if let Some(program) = find_program(name, Platform::Windows, env, exists) {
            let args = if name == "cmd" {
                os_args(&CMD_ARGS)
            } else {
                os_args(&POWERSHELL_ARGS)
            };
            return ShellSpec { program, args };
        }
    }
    let configured = non_empty(env, COMSPEC_ENV);
    let program = configured.unwrap_or_else(|| OsString::from(CMD_EXE));
    ShellSpec {
        program,
        args: os_args(&CMD_ARGS),
    }
}

/// Locates `name` as an executable and returns the program to spawn.
///
/// A name that contains a path separator is checked directly. Any other name is joined with
/// every `PATH` entry in order, probing the bare name first and then each `PATHEXT`
/// extension on a Windows host. The returned program is the queried name, which the child
/// process resolves through the inherited `PATH`.
fn find_program(
    name: &str,
    platform: Platform,
    env: &dyn Fn(&str) -> Option<OsString>,
    exists: &dyn Fn(&str) -> bool,
) -> Option<OsString> {
    if name.contains(['/', '\\']) {
        if exists(name) {
            return Some(OsString::from(name));
        }
        return None;
    }
    let raw_path = non_empty(env, PATH_ENV)?;
    let path = raw_path.to_string_lossy();
    let extensions = match platform {
        Platform::Unix => Vec::new(),
        Platform::Windows => pathext_entries(env),
    };
    for dir in path.split(platform.path_separator()) {
        if dir.is_empty() {
            continue;
        }
        let candidate = join(dir, name, platform);
        if exists(&candidate) {
            return Some(OsString::from(name));
        }
        for extension in &extensions {
            if exists(&format!("{candidate}{extension}")) {
                return Some(OsString::from(name));
            }
        }
    }
    None
}

/// `PATHEXT` extensions probed on a Windows host, defaulting to `DEFAULT_PATHEXT` when the
/// variable is unset or empty.
fn pathext_entries(env: &dyn Fn(&str) -> Option<OsString>) -> Vec<String> {
    let configured = non_empty(env, PATHEXT_ENV);
    let raw = configured.unwrap_or_else(|| OsString::from(DEFAULT_PATHEXT));
    let raw = raw.to_string_lossy();
    raw.split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Joins a `PATH` entry with a program name using the platform's directory separator.
fn join(dir: &str, name: &str, platform: Platform) -> String {
    let separator = platform.dir_separator();
    let dir = dir.strip_suffix(separator).unwrap_or(dir);
    format!("{dir}{separator}{name}")
}

/// Splits an override value on ASCII whitespace.
fn split_prefix_args(raw: &OsStr) -> Vec<OsString> {
    raw.to_string_lossy()
        .split_ascii_whitespace()
        .map(OsString::from)
        .collect()
}

/// Prefix arguments inferred from an override program's file stem, matched
/// case-insensitively.
fn inferred_prefix_args(program: &OsStr) -> Vec<OsString> {
    let stem = match Path::new(program).file_stem() {
        Some(stem) => stem.to_string_lossy().to_ascii_lowercase(),
        None => return os_args(&POSIX_ARGS),
    };
    match stem.as_str() {
        "pwsh" | "powershell" => os_args(&POWERSHELL_ARGS),
        "cmd" => os_args(&CMD_ARGS),
        _ => os_args(&POSIX_ARGS),
    }
}

/// Reads `key` through `env`, treating an unset or empty value as absent.
fn non_empty(env: &dyn Fn(&str) -> Option<OsString>, key: &str) -> Option<OsString> {
    env(key).filter(|value| !value.is_empty())
}

/// Builds a spec from a program name and string prefix arguments.
fn make_spec(program: &str, args: &[&str]) -> ShellSpec {
    ShellSpec {
        program: OsString::from(program),
        args: os_args(args),
    }
}

/// Clones string prefix arguments into owned OS strings.
fn os_args(args: &[&str]) -> Vec<OsString> {
    args.iter().map(|arg| OsString::from(*arg)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// Prefix arguments every PowerShell invocation carries.
    const POWERSHELL_PREFIX: [&str; 3] = ["-NoLogo", "-NoProfile", "-Command"];

    /// Builds the injected environment lookup.
    fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, OsString> {
        let mut env = BTreeMap::new();
        for (key, value) in pairs {
            env.insert((*key).to_string(), OsString::from(*value));
        }
        env
    }

    /// Case-insensitive fake filesystem, mirroring how a Windows host resolves names.
    fn fake_fs(paths: &[&str]) -> Vec<String> {
        let mut fs = Vec::with_capacity(paths.len());
        for path in paths {
            fs.push(path.to_ascii_lowercase());
        }
        fs
    }

    /// Runs `resolve` against an injected environment map and fake filesystem.
    fn resolve_with(
        platform: Platform,
        env: &BTreeMap<String, OsString>,
        fs: &[String],
    ) -> ShellSpec {
        let lookup = |key: &str| env.get(key).cloned();
        let exists = |candidate: &str| fs.contains(&candidate.to_ascii_lowercase());
        resolve(platform, &lookup, &exists)
    }

    /// Rendered prefix arguments of a spec.
    fn args(spec: &ShellSpec) -> Vec<String> {
        let mut rendered = Vec::with_capacity(spec.args.len());
        for arg in &spec.args {
            rendered.push(arg.to_string_lossy().into_owned());
        }
        rendered
    }

    #[test]
    fn windows_branch_resolves_pwsh_on_a_non_windows_host() {
        // `Platform::Windows` is injected, so the Windows branch runs on a Unix test host.
        let env = env_of(&[
            ("PATH", "C:\\bin;C:\\Windows\\System32"),
            ("PATHEXT", ".COM;.EXE"),
        ]);
        let spec = resolve_with(Platform::Windows, &env, &fake_fs(&["c:\\bin\\pwsh.exe"]));
        assert_eq!(spec.program, OsString::from("pwsh"));
        assert_eq!(args(&spec), POWERSHELL_PREFIX);
        assert_eq!(spec.display(), "pwsh -NoLogo -NoProfile -Command");
    }

    #[test]
    fn windows_prefers_pwsh_over_powershell_and_cmd() {
        let env = env_of(&[("PATH", "C:\\bin"), ("PATHEXT", ".EXE")]);
        let fs = fake_fs(&[
            "c:\\bin\\pwsh.exe",
            "c:\\bin\\powershell.exe",
            "c:\\bin\\cmd.exe",
        ]);
        let spec = resolve_with(Platform::Windows, &env, &fs);
        assert_eq!(spec.program, OsString::from("pwsh"));
    }

    #[test]
    fn windows_falls_back_to_powershell_without_pwsh() {
        let env = env_of(&[("PATH", "C:\\bin"), ("PATHEXT", ".EXE")]);
        let fs = fake_fs(&["c:\\bin\\powershell.exe", "c:\\bin\\cmd.exe"]);
        let spec = resolve_with(Platform::Windows, &env, &fs);
        assert_eq!(spec.program, OsString::from("powershell"));
        assert_eq!(args(&spec), POWERSHELL_PREFIX);
    }

    #[test]
    fn windows_falls_back_to_cmd_when_only_cmd_exists() {
        let env = env_of(&[("PATH", "C:\\bin"), ("PATHEXT", ".EXE;.CMD")]);
        let spec = resolve_with(Platform::Windows, &env, &fake_fs(&["c:\\bin\\cmd.exe"]));
        assert_eq!(spec.program, OsString::from("cmd"));
        assert_eq!(args(&spec), ["/C"]);
        assert_eq!(spec.display(), "cmd /C");
    }

    #[test]
    fn windows_falls_back_to_comspec_when_no_candidate_is_on_path() {
        let env = env_of(&[
            ("PATH", "C:\\bin"),
            ("PATHEXT", ".EXE"),
            ("COMSPEC", "D:\\Windows\\system32\\cmd.exe"),
        ]);
        let spec = resolve_with(Platform::Windows, &env, &fake_fs(&["c:\\bin\\other.exe"]));
        let comspec = OsString::from("D:\\Windows\\system32\\cmd.exe");
        assert_eq!(spec.program, comspec);
        assert_eq!(args(&spec), ["/C"]);
    }

    #[test]
    fn windows_falls_back_to_cmd_exe_when_comspec_is_unset_or_empty() {
        let unset = env_of(&[("PATH", "C:\\bin"), ("PATHEXT", ".EXE")]);
        let spec = resolve_with(Platform::Windows, &unset, &[]);
        assert_eq!(spec.program, OsString::from("cmd.exe"));
        assert_eq!(args(&spec), ["/C"]);
        let empty = env_of(&[("PATH", "C:\\bin"), ("PATHEXT", ".EXE"), ("COMSPEC", "")]);
        let spec = resolve_with(Platform::Windows, &empty, &[]);
        assert_eq!(spec.program, OsString::from("cmd.exe"));
    }

    #[test]
    fn unix_default_is_sh_dash_c_and_never_probes_path() {
        let env = env_of(&[("PATH", "/usr/bin:/bin")]);
        let probed = RefCell::new(Vec::new());
        let exists = |candidate: &str| {
            probed.borrow_mut().push(candidate.to_string());
            false
        };
        let lookup = |key: &str| env.get(key).cloned();
        let spec = resolve(Platform::Unix, &lookup, &exists);
        assert_eq!(spec.program, OsString::from("sh"));
        assert_eq!(args(&spec), ["-c"]);
        assert_eq!(spec.display(), "sh -c");
        assert!(probed.take().is_empty(), "the Unix default probes no PATH");
    }

    #[test]
    fn theway_shell_with_theway_shell_args_wins_on_every_platform() {
        let env = env_of(&[
            ("THEWAY_SHELL", "bash"),
            ("THEWAY_SHELL_ARGS", "  -l\t--norc "),
            ("PATH", "/usr/bin:/bin"),
        ]);
        let unix = resolve_with(Platform::Unix, &env, &fake_fs(&["/usr/bin/bash"]));
        assert_eq!(unix.program, OsString::from("bash"));
        assert_eq!(args(&unix), ["-l", "--norc"]);
        let windows = resolve_with(Platform::Windows, &env, &[]);
        assert_eq!(windows.program, OsString::from("bash"));
        assert_eq!(args(&windows), ["-l", "--norc"]);
    }

    #[test]
    fn an_override_never_probes_path() {
        let env = env_of(&[("THEWAY_SHELL", "bash"), ("PATH", "C:\\bin")]);
        let probed = RefCell::new(Vec::new());
        let exists = |candidate: &str| {
            probed.borrow_mut().push(candidate.to_string());
            false
        };
        let lookup = |key: &str| env.get(key).cloned();
        let spec = resolve(Platform::Windows, &lookup, &exists);
        assert_eq!(spec.program, OsString::from("bash"));
        assert!(probed.take().is_empty(), "an override probes no PATH");
    }

    #[test]
    fn theway_shell_alone_infers_prefix_args_from_the_file_stem() {
        let cases: [(&str, &[&str]); 5] = [
            ("pwsh", &POWERSHELL_PREFIX[..]),
            ("PWSH.EXE", &POWERSHELL_PREFIX[..]),
            ("/opt/powershell/bin/pwsh", &POWERSHELL_PREFIX[..]),
            ("powershell.exe", &POWERSHELL_PREFIX[..]),
            ("cmd", &CMD_ARGS[..]),
        ];
        for (program, expected) in cases {
            let env = env_of(&[("THEWAY_SHELL", program)]);
            let spec = resolve_with(Platform::Unix, &env, &[]);
            assert_eq!(spec.program, OsString::from(program));
            assert_eq!(args(&spec), expected, "program {program}");
        }
        let other = env_of(&[("THEWAY_SHELL", "fish")]);
        assert_eq!(args(&resolve_with(Platform::Unix, &other, &[])), ["-c"]);
    }

    #[test]
    fn empty_overrides_fall_back_to_the_platform_default() {
        let no_shell = env_of(&[("THEWAY_SHELL", ""), ("THEWAY_SHELL_ARGS", "-x")]);
        let spec = resolve_with(Platform::Unix, &no_shell, &[]);
        assert_eq!(spec, make_spec("sh", &POSIX_ARGS));
        let empty_args = env_of(&[("THEWAY_SHELL", "pwsh"), ("THEWAY_SHELL_ARGS", "")]);
        let spec = resolve_with(Platform::Unix, &empty_args, &[]);
        assert_eq!(args(&spec), POWERSHELL_PREFIX);
    }

    #[test]
    fn windows_probe_tries_the_bare_name_then_each_pathext_entry() {
        let env = env_of(&[("PATH", "C:\\first;C:\\second"), ("PATHEXT", ".COM;.BAT")]);
        let probed = RefCell::new(Vec::new());
        let exists = |candidate: &str| {
            probed.borrow_mut().push(candidate.to_string());
            false
        };
        let lookup = |key: &str| env.get(key).cloned();
        let spec = resolve(Platform::Windows, &lookup, &exists);
        assert_eq!(spec.program, OsString::from("cmd.exe"));
        let probed = probed.take();
        let head: Vec<&str> = probed.iter().take(3).map(String::as_str).collect();
        let expected = [
            "C:\\first\\pwsh",
            "C:\\first\\pwsh.COM",
            "C:\\first\\pwsh.BAT",
        ];
        assert_eq!(head, expected);
        assert!(probed.contains(&"C:\\second\\cmd.BAT".to_string()));
        assert_eq!(
            probed.len(),
            18,
            "three candidates times two directories times three probes"
        );
    }

    #[test]
    fn windows_probe_uses_the_default_extensions_when_pathext_is_unset() {
        let env = env_of(&[("PATH", "C:\\bin")]);
        let probed = RefCell::new(Vec::new());
        let exists = |candidate: &str| {
            probed.borrow_mut().push(candidate.to_string());
            false
        };
        let lookup = |key: &str| env.get(key).cloned();
        let spec = resolve(Platform::Windows, &lookup, &exists);
        assert_eq!(spec.program, OsString::from("cmd.exe"));
        let probed = probed.take();
        let head: Vec<&str> = probed.iter().take(5).map(String::as_str).collect();
        let expected = [
            "C:\\bin\\pwsh",
            "C:\\bin\\pwsh.COM",
            "C:\\bin\\pwsh.EXE",
            "C:\\bin\\pwsh.BAT",
            "C:\\bin\\pwsh.CMD",
        ];
        assert_eq!(head, expected);
    }

    #[test]
    fn empty_path_entries_are_not_probed() {
        let env = env_of(&[("PATH", ";"), ("PATHEXT", ".EXE")]);
        let fs = fake_fs(&["\\pwsh.exe", "pwsh.exe"]);
        let spec = resolve_with(Platform::Windows, &env, &fs);
        assert_eq!(spec.program, OsString::from("cmd.exe"));
    }

    #[test]
    fn a_program_name_with_a_separator_is_checked_directly() {
        let env = env_of(&[]);
        let lookup = |key: &str| env.get(key).cloned();
        let fs = fake_fs(&["c:\\tools\\sh.exe"]);
        let exists = |candidate: &str| fs.contains(&candidate.to_ascii_lowercase());
        let found = find_program("C:\\tools\\sh.exe", Platform::Windows, &lookup, &exists);
        assert_eq!(found, Some(OsString::from("C:\\tools\\sh.exe")));
        let missing = find_program(
            "C:\\tools\\missing.exe",
            Platform::Windows,
            &lookup,
            &exists,
        );
        assert_eq!(missing, None);
    }

    #[test]
    fn unix_probe_joins_path_entries_with_slashes() {
        let env = env_of(&[("PATH", "/usr/bin:/bin")]);
        let lookup = |key: &str| env.get(key).cloned();
        let fs = fake_fs(&["/bin/sh"]);
        let exists = |candidate: &str| fs.contains(&candidate.to_ascii_lowercase());
        let found = find_program("sh", Platform::Unix, &lookup, &exists);
        assert_eq!(found, Some(OsString::from("sh")));
        let missing = find_program("bash", Platform::Unix, &lookup, &exists);
        assert_eq!(missing, None);
    }

    #[test]
    fn a_path_entry_with_a_trailing_separator_is_not_doubled() {
        let env = env_of(&[("PATH", "C:\\bin\\"), ("PATHEXT", ".EXE")]);
        let fs = fake_fs(&["c:\\bin\\pwsh.exe"]);
        let spec = resolve_with(Platform::Windows, &env, &fs);
        assert_eq!(spec.program, OsString::from("pwsh"));
    }

    #[test]
    fn command_args_appends_the_command_after_the_prefix() {
        let sh = make_spec("sh", &POSIX_ARGS);
        let expected = vec![OsString::from("-c"), OsString::from("echo hi")];
        assert_eq!(sh.command_args("echo hi"), expected);
        let bare = make_spec("cmd.exe", &[]);
        assert_eq!(bare.command_args("dir"), vec![OsString::from("dir")]);
    }

    #[test]
    fn display_renders_the_program_then_its_prefix_arguments() {
        assert_eq!(make_spec("sh", &POSIX_ARGS).display(), "sh -c");
        assert_eq!(make_spec("cmd", &CMD_ARGS).display(), "cmd /C");
        assert_eq!(make_spec("cmd.exe", &[]).display(), "cmd.exe");
        let pwsh = make_spec("pwsh", &POWERSHELL_ARGS);
        assert_eq!(pwsh.display(), "pwsh -NoLogo -NoProfile -Command");
    }
}
