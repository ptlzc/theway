//! Prompt-template discovery for the CLI. Same dual-root precedence as the skills loader:
//! `<cwd>/.theway/templates/` overrides `~/.theway/templates/` on a name collision.
//!
//! The loader half (`load_templates` + frontmatter parsing) lives here rather than in
//! `theway-core`: file-based template discovery is a CLI concern — the core agent runtime
//! only consumes already-loaded `PromptTemplate` values (interpolation happens on the
//! type itself via `PromptTemplate::interpolate`).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use theway_core::{
    ExecutionEnv, FileErrorCode, FileKind, NativeEnv, PromptTemplate, SkillDiagnostic,
    SkillDiagnosticCode,
};
use tokio_util::sync::CancellationToken;

use theway::config::base_dir;

pub struct LoadedTemplates {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

pub async fn load_all(cwd: &Path) -> LoadedTemplates {
    let project: PathBuf = cwd.join(".theway").join("templates");
    let user: PathBuf = base_dir().join("templates");
    let env = NativeEnv::new(cwd.to_string_lossy().to_string());
    let cancel = CancellationToken::new();

    let mut combined: Vec<PromptTemplate> = Vec::new();
    let mut diagnostics = Vec::new();

    for dir in [user, project] {
        let s = dir.to_string_lossy().to_string();
        let LoadTemplatesOutput {
            templates,
            diagnostics: diags,
        } = load_templates(&env, &[s.as_str()], cancel.clone()).await;
        diagnostics.extend(diags);
        for t in templates {
            if let Some(i) = combined.iter().position(|x| x.name == t.name) {
                combined[i] = t;
            } else {
                combined.push(t);
            }
        }
    }
    LoadedTemplates {
        templates: combined,
        diagnostics,
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
/// Same diagnostic shape as the skills loader so the CLI can render both uniformly.
async fn load_templates(
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
    use super::parse_frontmatter;

    #[test]
    fn parses_frontmatter_name_and_description() {
        let raw = "---\nname: review\ndescription: code review checklist\n---\nBody {{var}}";
        let (fm, body) = parse_frontmatter(raw).unwrap();
        assert_eq!(fm.name.as_deref(), Some("review"));
        assert_eq!(fm.description.as_deref(), Some("code review checklist"));
        assert_eq!(body, "Body {{var}}");
    }
}
