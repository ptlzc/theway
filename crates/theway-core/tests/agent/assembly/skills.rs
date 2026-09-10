//! Skills catalog, prompt templates, and `reload_skills_from_disk` application.

use super::*;

#[tokio::test]
async fn reload_skills_from_disk_errors_when_not_configured() {
    let h = harness();
    let err = h.reload_skills_from_disk().await.unwrap_err();
    assert!(matches!(err, ReloadSkillsError::NotConfigured));
}

#[tokio::test]
async fn prompt_from_template_unknown_name_errors() {
    let h = harness();
    let err = h
        .prompt_from_template("missing", serde_json::Map::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown prompt template: missing"));
}

#[test]
fn replace_skills_updates_catalog_and_system_prompt() {
    let h = harness();
    h.replace_skills(vec![skill("a", "body")]);
    assert_eq!(h.skills().len(), 1);
    assert!(h.system_prompt().contains("test skill"));
    assert!(h.templates().is_empty());
}

#[tokio::test]
async fn reload_skills_from_disk_applies_loader_result() {
    let mut h = harness();
    h.reload_skills_fn = Some(Arc::new(|| {
        Box::pin(async move {
            LoadSkillsOutput {
                skills: vec![skill("reloaded", "body")],
                diagnostics: Vec::new(),
            }
        })
    }));

    let out = h.reload_skills_from_disk().await.unwrap();

    assert_eq!(out.skills.len(), 1);
    assert_eq!(h.skills().len(), 1);
    assert!(h.system_prompt().contains("reloaded"));
}
