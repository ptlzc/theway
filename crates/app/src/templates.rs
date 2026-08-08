//! Prompt-template discovery for the CLI. Same dual-root precedence as the skills loader:
//! `<cwd>/.theway/templates/` overrides `~/.theway/templates/` on a name collision.

use std::path::{Path, PathBuf};

use theway_core::{LoadTemplatesOutput, NativeEnv, PromptTemplate, load_templates};
use tokio_util::sync::CancellationToken;

use crate::config::base_dir;

pub struct LoadedTemplates {
    pub templates: Vec<PromptTemplate>,
    pub diagnostics: Vec<theway_core::SkillDiagnostic>,
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
