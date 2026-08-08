//! Prompt-template discovery + interpolation. Mirrors the skills loader (frontmatter at the
//! head, body below). Templates carry `{{var}}` placeholders; interpolation runs at
//! `prompt_from_template` time.
//!
//! Two halves:
//!   - **`PromptTemplateRegistry`** — in-memory lookup + interpolation; trivial.
//!   - **`load_templates`** — file-based discovery against an `ExecutionEnv`, with
//!     diagnostics. The CLI calls this against `<cwd>/.theway/templates/` and
//!     `~/.theway/templates/`, with project winning on a name collision.

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::types::{
    ExecutionEnv, FileErrorCode, FileKind, PromptTemplate, SkillDiagnostic, SkillDiagnosticCode,
};

pub struct PromptTemplateRegistry {
    templates: Vec<PromptTemplate>,
}

impl PromptTemplateRegistry {
    pub fn new(templates: Vec<PromptTemplate>) -> Self {
        Self { templates }
    }

    pub fn list(&self) -> &[PromptTemplate] {
        &self.templates
    }

    pub fn get(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.iter().find(|t| t.name == name)
    }

    /// Interpolate `{{var}}` placeholders. Missing keys leave the placeholder verbatim.
    pub fn interpolate(
        template: &PromptTemplate,
        vars: &serde_json::Map<String, serde_json::Value>,
    ) -> String {
        let mut out = template.content.clone();
        for (k, v) in vars {
            let needle = format!("{{{{{k}}}}}");
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                _ => v.to_string(),
            };
            out = out.replace(&needle, &value);
        }
        out
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// File loader
// ──────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
struct TemplateFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Default, Clone, Debug)]
pub struct LoadTemplatesOutput {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// Load templates from each directory. Missing directories are silently skipped. `.md` files at
/// the directory root become templates; directory recursion is intentionally *not* supported
/// (templates are flat to keep `/template <name>` unambiguous).
///
/// Same diagnostic shape as skills loader so the CLI can render both uniformly.
pub async fn load_templates(
    env: &dyn ExecutionEnv,
    dirs: &[&str],
    cancel: CancellationToken,
) -> LoadTemplatesOutput {
    let mut out = LoadTemplatesOutput::default();
    for dir in dirs {
        let info = match env.file_info(dir, cancel.clone()).await {
            Ok(i) => i,
            Err(e) => {
                if e.code != FileErrorCode::NotFound {
                    out.diagnostics.push(SkillDiagnostic {
                        code: SkillDiagnosticCode::FileInfoFailed,
                        message: e.message.clone(),
                        path: dir.to_string(),
                    });
                }
                continue;
            }
        };
        if !matches!(info.kind, FileKind::Directory) {
            continue;
        }
        let entries = match env.list_dir(dir, cancel.clone()).await {
            Ok(e) => e,
            Err(e) => {
                out.diagnostics.push(SkillDiagnostic {
                    code: SkillDiagnosticCode::ListFailed,
                    message: e.message,
                    path: dir.to_string(),
                });
                continue;
            }
        };
        for entry in entries {
            if !entry.name.ends_with(".md") {
                continue;
            }
            if !matches!(entry.kind, FileKind::File) {
                continue;
            }
            let raw = match env.read_text_file(&entry.path, cancel.clone()).await {
                Ok(t) => t,
                Err(e) => {
                    out.diagnostics.push(SkillDiagnostic {
                        code: SkillDiagnosticCode::ReadFailed,
                        message: e.message,
                        path: entry.path.clone(),
                    });
                    continue;
                }
            };
            let (frontmatter, body) = match parse_frontmatter(&raw) {
                Ok(parts) => parts,
                Err(msg) => {
                    out.diagnostics.push(SkillDiagnostic {
                        code: SkillDiagnosticCode::ParseFailed,
                        message: msg,
                        path: entry.path.clone(),
                    });
                    continue;
                }
            };
            let stem = entry
                .name
                .strip_suffix(".md")
                .unwrap_or(&entry.name)
                .to_string();
            let name = frontmatter.name.unwrap_or(stem);
            out.templates.push(PromptTemplate {
                name,
                description: frontmatter.description,
                content: body,
                file_path: entry.path,
            });
        }
    }
    out
}

fn parse_frontmatter(content: &str) -> Result<(TemplateFrontmatter, String), String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalized.starts_with("---") {
        return Ok((TemplateFrontmatter::default(), normalized));
    }
    let Some(end) = normalized[3..].find("\n---") else {
        return Ok((TemplateFrontmatter::default(), normalized));
    };
    let end = end + 3;
    let yaml = &normalized[4..end];
    let body = normalized[end + 4..].trim().to_string();
    let fm: TemplateFrontmatter = serde_yaml::from_str(yaml).map_err(|e| format!("yaml: {e}"))?;
    Ok((fm, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_known_vars_and_leaves_unknown() {
        let t = PromptTemplate {
            name: "t".into(),
            description: None,
            content: "hi {{who}} — {{missing}}".into(),
            file_path: "/x".into(),
        };
        let mut vars = serde_json::Map::new();
        vars.insert("who".into(), serde_json::json!("world"));
        assert_eq!(
            PromptTemplateRegistry::interpolate(&t, &vars),
            "hi world — {{missing}}"
        );
    }

    #[test]
    fn parses_frontmatter_name_and_description() {
        let raw = "---\nname: review\ndescription: code review checklist\n---\nBody {{var}}";
        let (fm, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(fm.name.as_deref(), Some("review"));
        assert_eq!(fm.description.as_deref(), Some("code review checklist"));
        assert_eq!(body, "Body {{var}}");
    }
}
