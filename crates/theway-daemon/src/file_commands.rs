//! Claude-code-format file commands (issue #37).
//!
//! Scans `.agents/commands` and `.claude/commands` (project, then user) for
//! `*.md` prompt files: the filename stem is the command name, an optional
//! YAML frontmatter block may carry `description`, and the body is the
//! prompt with `$ARGUMENTS` / `$1`..`$9` placeholders substituted at
//! dispatch time. On name collision the first root in priority order wins
//! (`.agents` before `.claude`, project before user).

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A loaded claude-code-format command file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileCommand {
    /// Command name (the markdown filename stem), invoked as `/name`.
    pub name: String,
    /// Frontmatter `description`, if any.
    pub description: String,
    /// Prompt body with placeholders still in place.
    pub body: String,
    /// Source file the command was loaded from.
    pub path: PathBuf,
}

#[derive(Default, Deserialize)]
struct CommandFrontmatter {
    #[serde(default)]
    description: String,
}

/// Ordered scan roots, highest priority first. `home` is the user home root
/// (issue #66: resolved at the CLI boundary as `DaemonPaths::home`).
pub fn command_dirs(cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        cwd.join(".agents").join("commands"),
        cwd.join(".claude").join("commands"),
        home.join(".agents").join("commands"),
        home.join(".claude").join("commands"),
    ]
}

/// Scan the default roots for `cwd` (see [`command_dirs`]). `home` is the
/// user home root resolved at the CLI boundary (issue #66:
/// `DaemonPaths::home`) — the kernel never reads `$HOME` itself.
pub fn scan_file_commands(cwd: &Path, home: &Path) -> Vec<FileCommand> {
    scan_file_commands_in(cwd, home)
}

/// Scan with an explicit user root (tests pin this to a tempdir so the
/// real `$HOME` never leaks into results).
pub fn scan_file_commands_in(cwd: &Path, user_root: &Path) -> Vec<FileCommand> {
    let dirs = [
        cwd.join(".agents").join("commands"),
        cwd.join(".claude").join("commands"),
        user_root.join(".agents").join("commands"),
        user_root.join(".claude").join("commands"),
    ];
    let mut commands: Vec<FileCommand> = Vec::new();
    for dir in dirs {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue, // missing dir: silently skipped
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
            .collect();
        files.sort();
        for path in files {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.is_empty() || stem.contains('/') || commands.iter().any(|c| c.name == stem) {
                // First root wins on name collision (issue #37).
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue; // unreadable file: skipped, never fatal
            };
            let (frontmatter, body) = parse_frontmatter(&raw);
            commands.push(FileCommand {
                name: stem.to_string(),
                description: frontmatter.description,
                body,
                path,
            });
        }
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands
}

/// Split a claude-code command file into `(frontmatter, body)`. Files
/// without a frontmatter block pass through unchanged.
fn parse_frontmatter(content: &str) -> (CommandFrontmatter, String) {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return (CommandFrontmatter::default(), normalized.trim().to_string());
    }
    let Some(end) = normalized[3..].find("\n---") else {
        return (CommandFrontmatter::default(), normalized.trim().to_string());
    };
    let end = end + 3;
    let frontmatter = serde_yaml::from_str(&normalized[4..end]).unwrap_or_default();
    let body = normalized[end + 4..].trim().to_string();
    (frontmatter, body)
}

/// Substitute claude-code placeholders in a command body: `$ARGUMENTS`
/// (alias `$ARGS`) with the raw argument tail, `$1`..`$9` with positional
/// arguments split from the tail.
pub fn expand_file_command(cmd: &FileCommand, args_tail: &str) -> String {
    let args: Vec<&str> = args_tail.split_whitespace().collect();
    let mut out = cmd.body.replace("$ARGUMENTS", args_tail);
    out = out.replace("$ARGS", args_tail);
    for (i, arg) in args.iter().enumerate().take(9) {
        out = out.replace(&format!("${}", i + 1), arg);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_command(root: &Path, dir: &str, name: &str, content: &str) {
        let d = root.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{name}.md")), content).unwrap();
    }

    #[test]
    fn scans_both_formats_with_frontmatter_description() {
        let cwd = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_command(
            cwd.path(),
            ".claude/commands",
            "review",
            "---\ndescription: review the diff\n---\nplease review $ARGUMENTS\n",
        );
        write_command(
            cwd.path(),
            ".agents/commands",
            "bare",
            "no frontmatter here\n",
        );
        let cmds = scan_file_commands_in(cwd.path(), user.path());
        assert_eq!(cmds.len(), 2);
        let review = cmds.iter().find(|c| c.name == "review").unwrap();
        assert_eq!(review.description, "review the diff");
        assert_eq!(review.body, "please review $ARGUMENTS");
        assert!(review.path.ends_with("review.md"));
        assert_eq!(cmds[0].name, "bare");
        assert_eq!(cmds[1].name, "review");
    }

    #[test]
    fn first_root_wins_on_name_collision() {
        let cwd = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        // Same name in .agents (project) and .claude (project+user):
        // the .agents copy is loaded, the rest are dropped.
        write_command(cwd.path(), ".agents/commands", "commit", "from agents");
        write_command(cwd.path(), ".claude/commands", "commit", "from claude");
        write_command(
            user.path(),
            ".claude/commands",
            "commit",
            "from user claude",
        );
        let cmds = scan_file_commands_in(cwd.path(), user.path());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].body, "from agents");
        assert!(cmds[0].path.starts_with(cwd.path().join(".agents")));
    }

    #[test]
    fn project_wins_over_user() {
        let cwd = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_command(user.path(), ".claude/commands", "deploy", "user");
        write_command(cwd.path(), ".claude/commands", "deploy", "project");
        let cmds = scan_file_commands_in(cwd.path(), user.path());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].body, "project");
    }

    #[test]
    fn missing_dirs_and_junk_files_are_skipped() {
        let cwd = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        write_command(cwd.path(), ".claude/commands", "ok", "body");
        std::fs::write(cwd.path().join(".claude/commands/notes.txt"), "junk").unwrap();
        std::fs::create_dir_all(cwd.path().join(".claude/commands/subdir")).unwrap();
        let cmds = scan_file_commands_in(cwd.path(), user.path());
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].name, "ok");
    }

    #[test]
    fn expand_substitutes_arguments_and_positionals() {
        let cmd = FileCommand {
            name: "commit".into(),
            description: String::new(),
            body: "commit $1 with $2\nrest: $ARGUMENTS\nshort: $ARGS".into(),
            path: PathBuf::new(),
        };
        let out = expand_file_command(&cmd, "one two three");
        assert_eq!(
            out,
            "commit one with two\nrest: one two three\nshort: one two three"
        );
        let no_args = expand_file_command(&cmd, "");
        assert_eq!(no_args, "commit $1 with $2\nrest: \nshort: ");
    }
}
