//! `remove_skill` builtin tool (skill-lifecycle task #23, S-A2b): delete a **user-installed**
//! skill from `~/.theway/skills/` and hot-reload the catalog.
//!
//! Scope guard (locked with Provider/Auth + QA on the #ux skill-lifecycle thread): only
//! `SkillSource::User` skills can be removed. A `Builtin` skill is compiled into the binary;
//! a `Project` skill belongs to the repo, not this user. Removing those isn't meaningful here
//! — the tool returns a bounded error pointing at `set_skill_state` / `/skills disable` instead.
//! This keeps "remove" strictly a deletion of something the user installed.
//!
//! Safety:
//! - Two-phase: first call (without `confirm: true`) previews the target path; `confirm: true`
//!   deletes. Same `ControlPlaneWrite` tier + interim two-phase guard as the other skill
//!   control-plane tools (the runtime user-Prompt path is the shared follow-up).
//! - The deletion target is derived from the resolved skill's `file_path` and must be a direct
//!   child of `~/.theway/skills/` — never a caller-supplied path component — so a hostile name
//!   can't escape the skills root.
//! - After deleting, the skill's `{User, name}` overlay entry is cleared so a later reinstall
//!   of the same name doesn't inherit a stale disabled state.
//! - Audit: `Custom { custom_type: "skill_control_plane" }`, op `remove`, with name/source/
//!   bounded path preview — no skill body.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{Value, json};
use theway_core::{
    AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, PermissionClassification,
    SkillSource, ToolExecutionMode,
};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;

use super::set_skill_state::default_base_dir;
use super::skill::SkillHarnessCell;
use crate::skill_overrides;

pub struct RemoveSkillTool {
    harness: SkillHarnessCell,
    /// The theway base dir (`~/.theway`). The skills root is `base_dir/skills`. Injected so tests
    /// operate on a temp dir, never the user's real home.
    base_dir: PathBuf,
}

impl RemoveSkillTool {
    pub fn new(harness: SkillHarnessCell) -> Self {
        Self::with_base_dir(harness, default_base_dir())
    }

    pub fn with_base_dir(harness: SkillHarnessCell, base_dir: PathBuf) -> Self {
        Self { harness, base_dir }
    }

    fn skills_root(&self) -> PathBuf {
        self.base_dir.join("skills")
    }
}

#[derive(Debug, Deserialize)]
struct Input {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    confirm: bool,
}

#[async_trait]
impl AgentTool for RemoveSkillTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "remove_skill"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        // Deletes a directory + reloads the catalog — serialize against other control-plane
        // writes in the same turn.
        Some(ToolExecutionMode::Sequential)
    }

    /// Issue #110 sub-PR 3 classifier — removing a user skill is a destructive
    /// control-plane write the model cannot self-authorize. Always route through the prompt
    /// channel. The bounded reason includes the skill name (which the user already typed
    /// into a prior session or saw in `/skills`) so the prompt card is decision-useful.
    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        let name = prepared_args
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unknown>");
        PermissionClassification::Prompt {
            reason: format!("remove user skill `{name}`"),
        }
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let input: Input = serde_json::from_value(params)
            .map_err(|e| AgentToolError::Message(format!("invalid arguments: {e}")))?;

        let harness = self
            .harness
            .get()
            .ok_or_else(|| AgentToolError::from("remove_skill not yet initialized"))?;

        // Resolve the active skill by name (catalog deduped by name).
        let skills = harness.skills();
        let Some(skill) = skills.iter().find(|s| s.name == input.name) else {
            let mut names: Vec<&str> = skills
                .iter()
                .filter(|s| s.name.starts_with(&input.name) || s.name.contains(&input.name))
                .map(|s| s.name.as_str())
                .take(5)
                .collect();
            names.dedup();
            let hint = if names.is_empty() {
                String::new()
            } else {
                format!(" Did you mean: {}?", names.join(", "))
            };
            return Err(AgentToolError::Message(format!(
                "no loaded skill named '{}'. Run /skills to list loaded skills.{hint}",
                input.name
            )));
        };
        let source = skill.source;

        // Scope guard: only user-installed skills are removable.
        if source != SkillSource::User {
            return Err(AgentToolError::Message(format!(
                "'{}' is a {} skill and cannot be removed (builtin skills are compiled in; \
                 project skills belong to the repo). Disable it instead with set_skill_state \
                 or `/skills disable {}`.",
                input.name,
                source.label(),
                input.name
            )));
        }

        // Optional source pin must be `user` (the only removable source).
        if let Some(req) = &input.source {
            let req_src = parse_source(req)?;
            if req_src != SkillSource::User {
                return Err(AgentToolError::Message(format!(
                    "only user-installed skills can be removed; '{}' is a user skill, not '{}'.",
                    input.name,
                    req_src.label()
                )));
            }
        }

        // Derive the deletion target strictly from the resolved skill's file_path, which must
        // sit under the skills root. The target is the direct child of the skills root on that
        // path (the `<name>/` dir for a `<name>/SKILL.md` layout, or the bare `<x>.md` file for
        // a root-level skill). Never built from the caller-supplied name.
        let skills_root = self.skills_root();
        let target = match deletion_target(&skills_root, Path::new(&skill.file_path)) {
            Some(t) => t,
            None => {
                return Err(AgentToolError::Message(format!(
                    "refusing to remove '{}': its file ({}) is not under the user skills root \
                     ({}).",
                    input.name,
                    skill.file_path,
                    skills_root.display()
                )));
            }
        };

        if !input.confirm {
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(format!(
                    "preview only — call again with `confirm: true` to delete. \
                     skill={} source=user target={}",
                    input.name,
                    target.display()
                ))],
                details: json!({
                    "phase": "preview",
                    "name": input.name,
                    "source": "user",
                    "target_path": target.display().to_string(),
                }),
                terminate: None,
            });
        }

        // Delete the skill from disk.
        let removed_meta = tokio::fs::symlink_metadata(&target).await;
        match removed_meta {
            Ok(meta) if meta.is_dir() => {
                tokio::fs::remove_dir_all(&target).await.map_err(|e| {
                    AgentToolError::Message(format!("remove {}: {e}", target.display()))
                })?;
            }
            Ok(_) => {
                tokio::fs::remove_file(&target).await.map_err(|e| {
                    AgentToolError::Message(format!("remove {}: {e}", target.display()))
                })?;
            }
            Err(_) => {
                // Already gone on disk — treat as success (idempotent), still clean overlay +
                // reload so the catalog drops it.
            }
        }

        // Forget any disabled-state overlay entry for this skill so a future reinstall of the
        // same name starts fresh.
        if let Err(e) = skill_overrides::remove_and_save(&self.base_dir, &input.name, source).await
        {
            tracing::warn!(
                skill = %input.name,
                error = %e,
                "failed to clear skill-overrides overlay entry after remove"
            );
        }

        let reload = harness
            .reload_skills_from_disk()
            .await
            .map_err(|e| AgentToolError::Message(format!("reload after remove: {e}")))?;

        // The removed skill must not survive in the reloaded catalog (no stale entry).
        let still_present = reload
            .skills
            .iter()
            .any(|s| s.name == input.name && s.source == SkillSource::User);

        let audit = json!({
            "op": "remove",
            "actor": "tool",
            "name": input.name,
            "source": "user",
            "target_path": target.display().to_string(),
        });
        let audit_entry_id = match harness
            .session()
            .append_custom("skill_control_plane", Some(audit))
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    skill = %input.name,
                    error = %e,
                    "skill_control_plane audit write failed; removal itself succeeded"
                );
                None
            }
        };

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "removed skill '{}' (user). catalog now has {} skill(s).",
                input.name,
                reload.skills.len()
            ))],
            details: json!({
                "phase": "removed",
                "name": input.name,
                "source": "user",
                "target_path": target.display().to_string(),
                "still_present_after_reload": still_present,
                "total_skills_after": reload.skills.len(),
                "audit_entry_id": audit_entry_id,
            }),
            terminate: None,
        })
    }
}

/// Compute what to delete for a skill whose SKILL.md is `file_path`, given the skills root.
/// Returns the direct child of `skills_root` on the path (a `<name>/` dir or a root-level
/// `<x>.md` file), or `None` if `file_path` is not under `skills_root`. The returned path is
/// always `skills_root.join(<first component>)`, so it can never escape the root regardless of
/// what the skill record claims.
fn deletion_target(skills_root: &Path, file_path: &Path) -> Option<PathBuf> {
    let rel = file_path.strip_prefix(skills_root).ok()?;
    let first = rel.components().next()?;
    match first {
        std::path::Component::Normal(c) => Some(skills_root.join(c)),
        // Any non-normal leading component (`..`, root, prefix) means the path isn't a clean
        // child of the skills root — refuse.
        _ => None,
    }
}

fn parse_source(s: &str) -> Result<SkillSource, AgentToolError> {
    match s.to_ascii_lowercase().as_str() {
        "builtin" => Ok(SkillSource::Builtin),
        "user" => Ok(SkillSource::User),
        "project" => Ok(SkillSource::Project),
        _ => Err(AgentToolError::from(
            "invalid `source` (expected one of: builtin, user, project)",
        )),
    }
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "remove_skill".into(),
    description:
        "Delete a user-installed skill (from ~/.theway/skills/) and hot-reload the catalog. Only \
         user-installed skills can be removed — builtin skills are compiled into theway and \
         project skills belong to the repo; for those, disable instead via set_skill_state. \
         Two-phase: first call previews the target path; call again with `confirm: true` to \
         delete. Removing also clears any disabled-state overlay entry for the skill."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Exact skill name as shown in /skills."
            },
            "source": {
                "type": "string",
                "enum": ["builtin", "user", "project"],
                "description": "Optional. Must be `user` if given — only user-installed skills are removable."
            },
            "confirm": {
                "type": "boolean",
                "default": false,
                "description": "When false (default) returns a preview; when true performs the deletion."
            }
        },
        "required": ["name"],
        "additionalProperties": false
    }),
});

// The suite removes skills through `NativeEnv` (direct host FS), which is compiled
// out of sandbox-only builds (issue #64), so the bridge compiles only with `local`.
#[cfg(all(test, feature = "local"))]
// Test files live in `tests/tools/remove_skill/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools/remove_skill");

#[cfg(all(test, feature = "local"))]
mod remove_skill_extra {
    tests_bridge_macro::tests_bridge!("tools/remove_skill/extra");
}
