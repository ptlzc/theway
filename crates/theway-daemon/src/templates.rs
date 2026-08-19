//! Prompt-template discovery for the CLI. Same dual-root precedence as the skills loader:
//! `<cwd>/.theway/templates/` overrides `~/.theway/templates/` on a name collision.
//!
//! The loader half (`load_templates` + frontmatter parsing) lives here rather than in
//! `theway-core`: file-based template discovery is a CLI concern — the core agent runtime
//! only consumes already-loaded `PromptTemplate` values (interpolation happens on the
//! type itself via `PromptTemplate::interpolate`).

use std::path::Path;
#[cfg(feature = "local")]
use std::path::PathBuf;

#[cfg(feature = "local")]
use serde::Deserialize;
#[cfg(feature = "local")]
use theway_core::{ExecutionEnv, FileErrorCode, FileKind, SkillDiagnosticCode};
use theway_core::{PromptTemplate, SkillDiagnostic};
#[cfg(feature = "local")]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "local")]
use crate::env::native::NativeEnv;
#[cfg(feature = "local")]
use theway_transport::client::base_dir;

pub struct LoadedTemplates {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

/// `sandbox`-only builds return empty: local template discovery walks the OS
/// filesystem via [`NativeEnv`], which is a `local`-feature capability
/// (daemon-kernel-layers).
#[cfg(feature = "local")]
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

/// Sandbox-only stub (see the `local` impl above).
///
/// Sandbox builds must never degrade silently: template discovery is
/// unavailable without the `local` feature, so the composition root logs an
/// explicit warn instead of looking like an empty-but-healthy template set.
/// The once-per-startup semantics are guaranteed by the callers (the
/// composition root loads templates once), not by this stub.
#[cfg(not(feature = "local"))]
pub async fn load_all(_cwd: &Path) -> LoadedTemplates {
    tracing::warn!(
        "template discovery unavailable in sandbox build — loading no templates (the sandbox \
         feature has no local filesystem access)"
    );
    LoadedTemplates {
        templates: Vec::new(),
        diagnostics: Vec::new(),
    }
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// File loader
// ──────────────────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "local")]
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
#[cfg(feature = "local")]
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

#[cfg(feature = "local")]
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

#[cfg(all(test, feature = "local"))]
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

#[cfg(all(test, feature = "local"))]
mod templates_tests {
    //! Mirrored tests live in `tests/templates/`; wrapped because the
    //! top-level `mod tests` slot is already used by the inline parser test.
    tests_bridge_macro::tests_bridge!("templates");
}

#[cfg(all(test, feature = "local"))]
mod templates_extra_tests {
    //! Additional mirrored coverage lives in `tests/templates/extra/`.
    tests_bridge_macro::tests_bridge!("templates/extra");
}
