//! `skill_builder` builtin tool (issue #21).
//!
//! Authors a new user-global skill from structured fields. Where `install_skill` ingests a
//! complete, externally sourced `SKILL.md`, `skill_builder` owns the format: the model
//! supplies `name` / `description` / `instructions` (+ optional `examples`) and the tool
//! renders the canonical template, so every produced skill is loadable by construction and
//! the model never hand-assembles frontmatter.
//!
//! Safety model is inherited from `install_skill` and shares its code paths:
//!
//! - Two-phase `confirm` flow — the first call only validates and previews; nothing is
//!   written until an explicit `confirm: true` call (+ `overwrite: true` when a same-name
//!   skill exists with different content).
//! - Rendered content runs through the same `parse_and_validate_skill_md` used by
//!   `install_skill`, then the same atomic tempfile+rename write and catalog hot-reload.
//! - `PermissionClassification::Prompt` (control-plane write) with a bounded reason; the
//!   skill name enters the reason only after passing the kebab-case charset check.
//! - The audit entry (`skill_install`, `source_kind: "builder"`) carries metadata + hashes
//!   only — never the body.

use std::path::PathBuf;

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

use super::install_skill::{
    atomic_write_skill, default_skills_root, on_disk_skill_hash, parse_and_validate_skill_md,
};
use super::skill::SkillHarnessCell;

pub struct SkillBuilderTool {
    harness: SkillHarnessCell,
    /// `${THEWAY_DIR:-~/.theway}/skills` in production; explicit so tests target a temp dir.
    skills_root: PathBuf,
}

impl SkillBuilderTool {
    pub fn new(harness: SkillHarnessCell) -> Self {
        Self::with_skills_root(harness, default_skills_root())
    }

    pub fn with_skills_root(harness: SkillHarnessCell, skills_root: PathBuf) -> Self {
        Self {
            harness,
            skills_root,
        }
    }

    fn target_path(&self, name: &str) -> PathBuf {
        self.skills_root.join(name).join("SKILL.md")
    }
}

#[derive(Debug, Deserialize)]
struct BuildInput {
    name: String,
    description: String,
    instructions: String,
    #[serde(default)]
    examples: Option<String>,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    overwrite: bool,
}

/// Render the canonical `SKILL.md`. Frontmatter goes through `serde_yaml` so special
/// characters in `description` are escaped correctly; the description is collapsed to a
/// single line first (it is the catalog trigger line, not body text).
fn render_skill_md(
    name: &str,
    description: &str,
    instructions: &str,
    examples: Option<&str>,
) -> Result<String, AgentToolError> {
    let description = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        return Err(AgentToolError::from("description must not be empty"));
    }
    if instructions.trim().is_empty() {
        return Err(AgentToolError::from("instructions must not be empty"));
    }

    let mut frontmatter = serde_yaml::Mapping::new();
    frontmatter.insert("name".into(), name.into());
    frontmatter.insert("description".into(), description.into());
    let yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|e| AgentToolError::Message(format!("render frontmatter: {e}")))?;

    let mut out = format!(
        "---\n{yaml}---\n\n# {}\n\n## Instructions\n\n{}\n",
        title_from_name(name),
        instructions.trim()
    );
    if let Some(examples) = examples.map(str::trim).filter(|e| !e.is_empty()) {
        out.push_str(&format!("\n## Examples\n\n{examples}\n"));
    }
    Ok(out)
}

/// `code-review-checklist` → `Code Review Checklist`.
fn title_from_name(name: &str) -> String {
    name.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl AgentTool for SkillBuilderTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "skill_builder"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        // Writes to the global skills directory and triggers a harness reload.
        Some(ToolExecutionMode::Sequential)
    }

    /// The preview phase is a pure read (render + validate, no fs writes), so it runs
    /// under `Allow` — the user-initiated "summarize recent work into a skill" flow costs
    /// exactly one approval, on the `confirm: true` write. That write is a persistent
    /// control-plane change growing the model's skill surface, so it always prompts. The
    /// name is model-supplied and only enters the bounded reason after passing the same
    /// charset shape the validator enforces. (Unlike install_skill — which prompts on
    /// preview too because it fetches untrusted external content — skill_builder's preview
    /// input is model-authored from the visible conversation.)
    fn permission_classification(&self, prepared_args: &Value) -> PermissionClassification {
        let confirm = prepared_args
            .get("confirm")
            .and_then(|c| c.as_bool())
            .unwrap_or(false);
        if !confirm {
            return PermissionClassification::Allow;
        }
        let name = prepared_args
            .get("name")
            .and_then(|n| n.as_str())
            .filter(|n| {
                !n.is_empty()
                    && n.len() <= 64
                    && n.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            })
            .unwrap_or("<invalid name>");
        PermissionClassification::Prompt {
            reason: format!("create user skill `{name}`"),
        }
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        _cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let input: BuildInput = serde_json::from_value(params)
            .map_err(|e| AgentToolError::Message(format!("invalid arguments: {e}")))?;

        // Phase 1: render + validate. The rendered content goes through the exact
        // validation install_skill applies, so authored skills can never diverge from what
        // the loader accepts. Pure read; no fs writes happen here.
        let rendered = render_skill_md(
            &input.name,
            &input.description,
            &input.instructions,
            input.examples.as_deref(),
        )?;
        let parsed = parse_and_validate_skill_md(&rendered)?;
        if parsed.name != input.name {
            return Err(AgentToolError::Message(format!(
                "skill name `{}` did not survive rendering; use lowercase kebab-case",
                input.name
            )));
        }
        let target_path = self.target_path(&parsed.name);
        let existing_hash = on_disk_skill_hash(&target_path).await;
        let existing = existing_hash.is_some();
        let overwrite_required = existing && existing_hash.as_deref() != Some(&parsed.content_hash);

        // Shadow warnings come from the live catalog: a same-name project skill takes
        // precedence over the new user skill; a same-name builtin is shadowed by it.
        let mut warnings = parsed.warnings.clone();
        if let Some(harness) = self.harness.get() {
            for skill in harness.skills() {
                if skill.name == parsed.name {
                    match skill.source {
                        SkillSource::Project => warnings.push(format!(
                            "a project skill named '{}' exists and will shadow this user skill",
                            parsed.name
                        )),
                        SkillSource::Builtin => warnings.push(format!(
                            "this will shadow the builtin skill '{}'",
                            parsed.name
                        )),
                        SkillSource::User => {}
                    }
                }
            }
        }

        if !input.confirm {
            return Ok(AgentToolResult {
                content: vec![UserContentBlock::text(format!(
                    "preview only — call again with `confirm: true` to create the skill. \
                     name={} target={} size={}B existing={} overwrite_required={}",
                    parsed.name,
                    target_path.display(),
                    parsed.size,
                    existing,
                    overwrite_required
                ))],
                details: json!({
                    "phase": "preview",
                    "name": parsed.name,
                    "description": parsed.description,
                    "warnings": warnings,
                    "target_path": target_path.display().to_string(),
                    "content_hash": parsed.content_hash,
                    "size": parsed.size,
                    "existing": existing,
                    "overwrite_required": overwrite_required,
                }),
                terminate: None,
            });
        }

        // Phase 2: write. Refuse silent overwrite unless caller explicitly asked.
        if overwrite_required && !input.overwrite {
            return Err(AgentToolError::Message(format!(
                "skill '{}' already exists with different content. Call again with \
                 `overwrite: true` to replace it.",
                parsed.name
            )));
        }

        atomic_write_skill(&target_path, &parsed.normalized_content).await?;

        let harness = self
            .harness
            .get()
            .ok_or_else(|| AgentToolError::from("skill_builder not yet initialized"))?;
        let reload = harness
            .reload_skills_from_disk()
            .await
            .map_err(|e| AgentToolError::Message(format!("reload after build: {e}")))?;

        let installed = reload.skills.iter().any(|s| s.name == parsed.name);
        warnings.extend(
            reload
                .diagnostics
                .iter()
                .filter(|d| {
                    d.path.contains(&parsed.name) || d.path == target_path.display().to_string()
                })
                .map(|d| format!("{:?}: {}", d.code, d.message)),
        );

        // Persistent audit: same `skill_install` channel as install_skill so resume and
        // forensics see one uniform record of model-driven skill writes. `source_kind:
        // "builder"` distinguishes authored skills; the body is never included.
        let audit_payload = json!({
            "status": "installed",
            "name": parsed.name,
            "target_path": target_path.display().to_string(),
            "source_kind": "builder",
            "source": Value::Null,
            "before_hash": existing_hash,
            "after_hash": parsed.content_hash,
            "size": parsed.size,
            "overwrote": overwrite_required,
            "idempotent": existing && !overwrite_required,
            "installed_visible_in_catalog": installed,
            "diagnostics_count": reload.diagnostics.len(),
            "warnings": warnings.clone(),
        });
        let audit_entry_id = match harness
            .session()
            .append_custom("skill_install", Some(audit_payload))
            .await
        {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(
                    skill = %parsed.name,
                    error = %e,
                    "skill_install audit write failed; the skill itself was created"
                );
                None
            }
        };

        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(format!(
                "created skill '{}' at {} ({}B). catalog now has {} skill(s).",
                parsed.name,
                target_path.display(),
                parsed.size,
                reload.skills.len()
            ))],
            details: json!({
                "phase": "installed",
                "name": parsed.name,
                "target_path": target_path.display().to_string(),
                "content_hash": parsed.content_hash,
                "size": parsed.size,
                "overwrote": overwrite_required,
                "total_skills_after": reload.skills.len(),
                "diagnostics_count": reload.diagnostics.len(),
                "warnings": warnings,
                "installed_visible_in_catalog": installed,
                "audit_entry_id": audit_entry_id,
            }),
            terminate: None,
        })
    }
}

static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "skill_builder".into(),
    description:
        "Create a NEW user skill from structured fields and hot-reload the catalog. Use this \
         when the user asks to create, save, or codify a reusable skill, workflow, checklist, \
         or convention — including \"summarize the recent work / this conversation into a \
         skill\": distill the generalizable workflow from the conversation (steps actually \
         performed, commands used, pitfalls hit) and write instructions for the general case, \
         not a transcript of this one instance. Use install_skill instead when installing an \
         existing SKILL.md from a URL, file, or pasted content. The tool renders canonical \
         SKILL.md (frontmatter + sections) from name/description/instructions — do not \
         hand-write frontmatter. Two-phase: first call without `confirm` validates and \
         returns a preview (target path, hash, size, shadow warnings); show the user the \
         planned name/description and get their go-ahead, then call again with `confirm: \
         true` to write atomically to ~/.theway/skills/<name>/SKILL.md and reload. A same-name \
         skill with different content additionally requires `overwrite: true`."
            .into(),
    parameters: json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Skill name: lowercase kebab-case (a-z, 0-9, hyphens), max 64 chars. Becomes the directory name and the /skill lookup key."
            },
            "description": {
                "type": "string",
                "description": "One-line summary of what the skill does AND when to use it (max 1024 chars). This is the trigger line the model sees in the catalog — include concrete cue phrases."
            },
            "instructions": {
                "type": "string",
                "description": "Markdown body: the steps, conventions, and guidance the skill teaches. Rendered under an '## Instructions' heading."
            },
            "examples": {
                "type": "string",
                "description": "Optional markdown examples, rendered under an '## Examples' heading."
            },
            "confirm": {
                "type": "boolean",
                "default": false,
                "description": "When false (default), validates and returns a preview without writing. When true, writes the skill and reloads the catalog."
            },
            "overwrite": {
                "type": "boolean",
                "default": false,
                "description": "Required when a skill of the same name already exists with different content."
            }
        },
        "required": ["name", "description", "instructions"],
        "additionalProperties": false
    }),
});

// The suite writes skills through `NativeEnv` (direct host FS), which is compiled
// out of sandbox-only builds (issue #64), so the bridge compiles only with `local`.
#[cfg(all(test, feature = "local"))]
// Test files live in `tests/tools/skill_builder/` (mirror of src), pulled in by
// path so they keep unit-test semantics (private access). See docs/rust-test-files.md.
tests_bridge_macro::tests_bridge!("tools/skill_builder");
