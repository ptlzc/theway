//! Tests for `agent::types` — split out of src (see docs/rust-test-files.md).

use super::super::*;

#[test]
fn file_error_new_and_with_path() {
    let err = FileError::new(FileErrorCode::NotFound, "missing");
    assert_eq!(err.code, FileErrorCode::NotFound);
    assert_eq!(err.message, "missing");
    assert_eq!(err.path, None);
    assert_eq!(err.to_string(), "missing");

    let with_path = err.with_path("/tmp/a");
    assert_eq!(with_path.path.as_deref(), Some("/tmp/a"));
}

#[test]
fn execution_error_new() {
    let err = ExecutionError::new(ExecutionErrorCode::Timeout, "slow");
    assert_eq!(err.code, ExecutionErrorCode::Timeout);
    assert_eq!(err.message, "slow");
    assert_eq!(err.to_string(), "slow");
}

#[test]
fn skill_source_labels() {
    assert_eq!(SkillSource::Builtin.label(), "builtin");
    assert_eq!(SkillSource::User.label(), "user");
    assert_eq!(SkillSource::Project.label(), "project");
}

#[test]
fn exec_options_default_is_empty() {
    let opts = ExecOptions::default();
    assert!(opts.cwd.is_none());
    assert!(opts.env.is_none());
    assert!(opts.timeout_secs.is_none());
    assert!(opts.abort.is_none());
    assert!(opts.on_stdout.is_none());
    assert!(opts.on_stderr.is_none());
}

#[test]
fn prompt_template_interpolates_non_string_values() {
    let t = PromptTemplate {
        name: "t".into(),
        description: None,
        content: "{{n}}".into(),
        file_path: "/x".into(),
    };
    let mut vars = serde_json::Map::new();
    vars.insert("n".into(), serde_json::json!(42));
    assert_eq!(t.interpolate(&vars), "42");
}

#[test]
fn skill_frontmatter_deserializes_snake_and_kebab_disable_flags() {
    let fm: SkillFrontmatter =
        serde_yaml::from_str("name: a\ndisable_model_invocation: true").unwrap();
    assert!(fm.disable_model_invocation);

    let fm: SkillFrontmatter =
        serde_yaml::from_str("name: a\ndisable-model-invocation: true").unwrap();
    assert!(fm.disable_model_invocation);
}
