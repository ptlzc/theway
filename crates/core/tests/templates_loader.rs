//! Test the prompt-template file loader. Verifies that two roots overlay properly and that
//! the loaded templates are usable via PromptTemplateRegistry interpolation.

use std::path::Path;

use tempfile::TempDir;
use theway_core::{LoadTemplatesOutput, NativeEnv, PromptTemplateRegistry, load_templates};
use tokio_util::sync::CancellationToken;

fn write(root: &Path, name: &str, frontmatter_desc: &str, body: &str) {
    std::fs::create_dir_all(root).unwrap();
    let content = format!("---\nname: {name}\ndescription: {frontmatter_desc}\n---\n{body}\n");
    std::fs::write(root.join(format!("{name}.md")), content).unwrap();
}

#[tokio::test]
async fn loads_templates_from_dual_roots_with_project_winning() {
    let home = TempDir::new().unwrap();
    let cwd = TempDir::new().unwrap();
    let user_root = home.path().join("templates");
    let project_root = cwd.path().join(".theway").join("templates");

    write(&user_root, "shared", "user", "User body {{var}}");
    write(&project_root, "shared", "project", "Project body {{var}}");
    write(&user_root, "only-user", "user-only", "Only user");

    let env = NativeEnv::new(cwd.path().to_string_lossy().to_string());
    let cancel = CancellationToken::new();
    // Load user first, then project, then dedupe with project winning.
    let user_path = user_root.to_string_lossy().to_string();
    let project_path = project_root.to_string_lossy().to_string();
    let LoadTemplatesOutput {
        templates: mut combined,
        diagnostics,
    } = load_templates(&env, &[user_path.as_str()], cancel.clone()).await;
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let project_load = load_templates(&env, &[project_path.as_str()], cancel.clone()).await;
    for t in project_load.templates {
        if let Some(i) = combined.iter().position(|x| x.name == t.name) {
            combined[i] = t;
        } else {
            combined.push(t);
        }
    }

    let names: Vec<&str> = combined.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"shared"));
    assert!(names.contains(&"only-user"));

    let shared = combined.iter().find(|t| t.name == "shared").unwrap();
    assert_eq!(shared.description.as_deref(), Some("project"));
    assert!(shared.content.contains("Project body"));

    // Interpolation round-trip.
    let mut vars = serde_json::Map::new();
    vars.insert("var".into(), serde_json::json!("world"));
    let rendered = PromptTemplateRegistry::interpolate(shared, &vars);
    assert_eq!(rendered, "Project body world");
}

#[tokio::test]
async fn missing_dirs_produce_no_diagnostics() {
    let cwd = TempDir::new().unwrap();
    let env = NativeEnv::new(cwd.path().to_string_lossy().to_string());
    let path = cwd.path().join("nope").to_string_lossy().to_string();
    let out = load_templates(&env, &[path.as_str()], CancellationToken::new()).await;
    assert!(out.templates.is_empty());
    assert!(out.diagnostics.is_empty(), "{:#?}", out.diagnostics);
}
