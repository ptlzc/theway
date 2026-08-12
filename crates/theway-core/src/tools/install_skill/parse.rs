//! Frontmatter parsing + validation for `InstallSkill` — parse the fetched `SKILL.md`
//! body, validate the skill name, normalize an oversized / missing description to a
//! bounded fallback, and produce the normalized content + SHA256 hash that both the
//! preview phase and the on-disk write use.
//!
//! `parse_and_validate_skill_md` is shared with `SkillBuilder` so authored and installed
//! skills obey identical rules.

use serde::Deserialize;
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};
use theway_core::AgentToolError;

use super::fetch::Fetched;
use super::{MAX_DESCRIPTION_LEN, MAX_NAME_LEN};

pub(crate) struct ParsedSkill {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) normalized_content: String,
    pub(crate) content_hash: String,
    pub(crate) size: usize,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Validate complete `SKILL.md` text (frontmatter + body) and normalize it. Shared with
/// `SkillBuilder`, which renders its own content and runs it through here so authored and
/// installed skills obey identical rules.
pub(crate) fn parse_and_validate_skill_md(content: &str) -> Result<ParsedSkill, AgentToolError> {
    parse_and_validate(&Fetched {
        content: content.to_string(),
    })
}

pub(super) fn parse_and_validate(fetched: &Fetched) -> Result<ParsedSkill, AgentToolError> {
    let normalized = fetched.content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Err(AgentToolError::from(
            "skill body missing YAML frontmatter (must start with `---` followed by name/description)",
        ));
    }
    let end = normalized[3..]
        .find("\n---")
        .ok_or_else(|| AgentToolError::from("skill frontmatter missing closing `\\n---`"))?;
    let yaml = &normalized[4..end + 3];
    let frontmatter: Frontmatter = serde_yaml::from_str(yaml)
        .map_err(|e| AgentToolError::Message(format!("invalid frontmatter yaml: {e}")))?;

    let name = frontmatter
        .name
        .ok_or_else(|| AgentToolError::from("frontmatter missing required field: name"))?;
    validate_name(&name)?;

    let (description, warnings, rewrite_description) =
        normalize_description(frontmatter.description);
    let normalized_content = if rewrite_description {
        normalize_skill_content(&normalized, end + 3, &description)?
    } else {
        normalized
    };

    let mut hasher = Sha256::new();
    hasher.update(normalized_content.as_bytes());
    let hash = hex::encode(hasher.finalize());
    let size = normalized_content.len();

    Ok(ParsedSkill {
        name,
        description,
        normalized_content,
        content_hash: hash,
        size,
        warnings,
    })
}

fn normalize_description(description: Option<String>) -> (String, Vec<String>, bool) {
    let Some(description) = description else {
        return (
            fallback_description(),
            vec!["description missing; using generated fallback".to_string()],
            true,
        );
    };
    let trimmed = description.trim().to_string();
    if trimmed.is_empty() {
        return (
            fallback_description(),
            vec!["description empty; using generated fallback".to_string()],
            true,
        );
    }
    if trimmed.chars().count() > MAX_DESCRIPTION_LEN {
        return (
            fallback_description(),
            vec![format!(
                "description exceeds {MAX_DESCRIPTION_LEN} characters; using generated fallback"
            )],
            true,
        );
    }
    (trimmed, Vec::new(), false)
}

fn fallback_description() -> String {
    "No description provided.".to_string()
}

fn normalize_skill_content(
    normalized: &str,
    yaml_end: usize,
    description: &str,
) -> Result<String, AgentToolError> {
    let yaml = &normalized[4..yaml_end];
    let mut frontmatter: YamlValue = serde_yaml::from_str(yaml)
        .map_err(|e| AgentToolError::Message(format!("invalid frontmatter yaml: {e}")))?;
    let mapping = frontmatter
        .as_mapping_mut()
        .ok_or_else(|| AgentToolError::from("skill frontmatter must be a YAML mapping"))?;
    mapping.insert(
        YamlValue::String("description".to_string()),
        YamlValue::String(description.to_string()),
    );
    let frontmatter = serde_yaml::to_string(&frontmatter)
        .map_err(|e| AgentToolError::Message(format!("failed to normalize frontmatter: {e}")))?;

    Ok(format!(
        "---\n{}{}",
        frontmatter.trim_start_matches("---\n"),
        &normalized[yaml_end..]
    ))
}

fn validate_name(name: &str) -> Result<(), AgentToolError> {
    if name.is_empty() {
        return Err(AgentToolError::from("skill name must not be empty"));
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(AgentToolError::Message(format!(
            "skill name exceeds {MAX_NAME_LEN} characters"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(AgentToolError::from(
            "skill name must contain only lowercase a-z, 0-9, and hyphens",
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(AgentToolError::from(
            "skill name must not start or end with a hyphen",
        ));
    }
    if name.contains("--") {
        return Err(AgentToolError::from(
            "skill name must not contain consecutive hyphens",
        ));
    }
    Ok(())
}
