use super::*;
use crate::agent::types::SkillSource;

fn skill(name: &str, description: &str) -> Skill {
    Skill {
        name: name.into(),
        description: description.into(),
        file_path: format!("/skills/{name}/SKILL.md"),
        content: "body".into(),
        disable_model_invocation: false,
        source: SkillSource::User,
    }
}

#[test]
fn format_skills_without_catalog_returns_empty_string() {
    assert_eq!(format_skills_for_system_prompt(&[]), "");
}

#[test]
fn format_skills_renders_each_catalog_entry() {
    let output =
        format_skills_for_system_prompt(&[skill("alpha", "first"), skill("beta", "second")]);

    assert!(output.starts_with("<skills>\n"));
    assert!(output.contains("- name: alpha\n  description: first\n"));
    assert!(output.contains("- name: beta\n  description: second\n"));
    assert!(output.ends_with("</skills>"));
}
