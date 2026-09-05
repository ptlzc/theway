use std::collections::{HashMap, HashSet};

use theway_transport::commands::Registry;
use theway_transport::wire::WireSkillSnapshot;

/// Assemble the slash-command completion list: the TUI-local command set from
/// `registry` (`local_commands::local_registry` — quit/clear/help + aliases) +
/// the TUI-local command set (`LOCAL_COMMANDS` — commands the client
/// intercepts and never forwards) + the daemon-side command surface (the
/// daemon owns the full registry; the client forwards slash text via
/// `send_message`) + one entry per enabled skill + the daemon-scanned
/// claude-code-format file commands (issue #37) + MCP tool reference entries.
///
/// Skill naming (issue #110): the dispatchable `/<shortcut>` is canonical
/// when unique and non-colliding; `/skill::<name>` is the fallback for
/// command collisions and ambiguous names, so a skill never appears twice.
/// Unknown slash commands submitted by the user fall back to a plain user
/// message (#37 semantics), so catalog entries are reference info.
pub(crate) fn collect_slash_commands(
    registry: &Registry,
    skills: &[WireSkillSnapshot],
    file_commands: &[String],
    mcp_tool_names: &[String],
) -> Vec<String> {
    let mut commands: Vec<String> = registry
        .commands()
        .iter()
        .flat_map(|c| {
            let mut names = vec![format!("/{}", c.name())];
            names.extend(c.aliases().iter().map(|a| format!("/{a}")));
            names
        })
        .collect();
    commands.extend(DAEMON_COMMANDS.iter().map(|name| format!("/{name}")));
    commands.extend(LOCAL_COMMANDS.iter().map(|name| format!("/{name}")));
    commands.extend(file_commands.iter().cloned());

    let mut seen: HashSet<String> = commands.iter().cloned().collect();
    let mut shortcut_counts: HashMap<String, usize> = HashMap::new();
    for skill in skills.iter().filter(|skill| skill.enabled) {
        if let Some(shortcut) = skill.name.split('/').next() {
            *shortcut_counts.entry(shortcut.to_string()).or_default() += 1;
        }
    }
    for skill in skills.iter().filter(|skill| skill.enabled) {
        let Some(shortcut) = skill.name.split('/').next() else {
            continue;
        };
        let shortcut = format!("/{shortcut}");
        let unique = shortcut_counts.get(&shortcut[1..]) == Some(&1);
        if unique && seen.insert(shortcut.clone()) {
            commands.push(shortcut);
        } else {
            // Issue #47/#110: exact-name fallback for colliding or ambiguous
            // skills; never add it when the shortcut was already canonical.
            commands.push(format!("/skill::{}", skill.name));
        }
    }
    // MCP catalog: one entry per connected MCP tool, names verbatim —
    // server-defined names are never rewritten (issue #47).
    for tool in mcp_tool_names {
        commands.push(format!("/mcp:{tool}"));
    }
    commands
}

/// Daemon-side slash commands the client forwards (the daemon's registry is not
/// exposed over RPC). Hint list only — completion, no dispatch. Keep in sync
/// with the commands `theway_daemon::Registry::with_daemon_commands()` registers
/// (crates/theway-daemon/src/commands/mod.rs), including the auth surface
/// (`/login` `/logout` `/sessions`) and `/fork` (issue #55). The TUI-local
/// commands (help/clear/quit/…) are NOT listed here: they come from the `registry`
/// argument above.
/// `crontab` is the daemon's alias for `/cron`.
pub(crate) const DAEMON_COMMANDS: &[&str] = &[
    "login",
    "logout",
    "sessions",
    "skills",
    "skill",
    "reload",
    "model",
    "thinking",
    "cost",
    "diag",
    "template",
    "save",
    "compact",
    "collapse",
    "undo",
    "bug-report",
    "name",
    "fork",
    "session",
    "web-connect",
    "web-disconnect",
    "share",
    "find",
    "history",
    "goal",
    "goal-start",
    "triggers",
    "new-trigger",
    "cron",
    "crontab",
    "inbox",
];

/// TUI-local slash commands (issues #52 + #54 + #56): dispatched in the
/// client, never forwarded to the daemon. NOT listed in `DAEMON_COMMANDS` —
/// the daemon has no `/new`, `/status-panel` or `/resume` command; the
/// client intercepts them (`/new` drives the session-resource RPCs,
/// `/status-panel` opens the local panel-mode menu, `/resume` opens the
/// session-list popup over `list_sessions`).
const LOCAL_COMMANDS: &[&str] = &[
    "new",
    "status-panel",
    "resume",
    "extensions",
    "extension-reload",
    "extension-trust",
];
