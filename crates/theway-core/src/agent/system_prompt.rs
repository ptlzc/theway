//! System-prompt builder. 1:1 port of `packages/agent/src/harness/system-prompt.ts`.
//!
//! Renders a discovered skill catalog into a discoverable block the model can scan to decide
//! which skill to invoke. The actual skill bodies are loaded on demand via the per-skill
//! invocation block (see [`crate::agent::skills::format_skill_invocation`]).

use super::types::Skill;

const SKILL_BLOCK_PREAMBLE: &[&str] = &[
    "The user has provided skills they want you to use whenever the user request can be solved with their help.",
    "Below is a list of skills with their unique names and descriptions of what they do.",
    "Use the `Skill` tool to invoke a skill by name when applicable.",
    "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.",
];

/// Render the skill catalog into a `<skills>` block for inclusion in the system prompt. Returns
/// the empty string when `skills` is empty so the surrounding prompt stays clean.
pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("<skills>\n");
    for line in SKILL_BLOCK_PREAMBLE {
        out.push_str(line);
        out.push('\n');
    }
    out.push('\n');
    for skill in skills {
        out.push_str(&format!(
            "- name: {}\n  description: {}\n",
            skill.name, skill.description
        ));
    }
    out.push_str("</skills>");
    out
}

#[cfg(test)]
tests_bridge_macro::tests_bridge!("agent/system_prompt");
