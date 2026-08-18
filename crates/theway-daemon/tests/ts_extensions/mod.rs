//! Mirrored tests for `ts_extensions` — private `TsExtension::new` failures,
//! registry sorting/error branches, and the compaction-adapter fallback
//! paths that the top-level integration tests don't drive.

use std::path::Path;
use std::sync::Arc;

use theway_core::agent::compaction::algorithm::CompactAlgorithm;
use theway_core::agent::compaction::compaction::DEFAULT_COMPACTION_SETTINGS;
use theway_core::agent::session::session::SessionTreeEntry;
use theway_core::types::AgentMessage;

use super::*;

fn write_extension(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let ext_dir = dir.join(".theway").join("extensions");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let path = ext_dir.join(format!("{name}.ts"));
    std::fs::write(&path, source).unwrap();
    path
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(theway_llm_provider::Message::User(
        theway_llm_provider::UserMessage {
            role: theway_llm_provider::UserRole::User,
            content: theway_llm_provider::UserContent::Text(text.into()),
            timestamp: 0,
        },
    ))
}

fn entries(n: usize) -> Vec<SessionTreeEntry> {
    (0..n)
        .map(|i| SessionTreeEntry::Message {
            id: format!("m{i}"),
            parent_id: None,
            timestamp: format!("{i}"),
            message: user_message(&format!("msg {i}")),
        })
        .collect()
}

#[test]
fn ts_extension_new_rejects_missing_kind_export() {
    let dir = tempfile::tempdir().unwrap();
    let source = "export const helper = 42;\n";
    let path = dir.path().join("no-kind.ts");

    let err = TsExtension::new("no-kind".into(), path.clone(), source.into())
        .err()
        .expect("missing kind must fail");

    assert!(err.contains("kind"), "{err}");
}

#[test]
fn ts_extension_new_rejects_parse_errors() {
    let dir = tempfile::tempdir().unwrap();
    let source = "export const kind = \"compaction\";\nexport function broken(ctx {";
    let path = dir.path().join("broken.ts");

    let err = TsExtension::new("broken".into(), path.clone(), source.into())
        .err()
        .expect("parse error must fail");

    assert!(err.contains("parse error"), "{err}");
}

#[test]
fn ts_extension_getters_return_constructor_values() {
    let dir = tempfile::tempdir().unwrap();
    let source = r#"export const kind = "compaction";
export const description = "getters";
"#;
    let ext = TsExtension::new("getters".into(), dir.path().join("getters.ts"), source.into()).unwrap();

    assert_eq!(ext.name(), "getters");
    assert_eq!(ext.kind(), "compaction");
    assert_eq!(ext.path(), Path::new(&dir.path().join("getters.ts")));
}

#[test]
fn registry_new_and_default_start_empty() {
    let registry = ExtensionRegistry::new();
    assert!(registry.names().is_empty());
    assert!(registry.get("missing").is_none());
    assert!(registry.by_kind("compaction").is_empty());
    assert!(registry.errors.is_empty());

    let default = ExtensionRegistry::default();
    assert!(default.names().is_empty());
}

#[test]
fn extension_dirs_project_precedes_user_base() {
    let dirs = ExtensionRegistry::extension_dirs(Path::new("/cwd"), Path::new("/base"));

    assert_eq!(dirs.len(), 2);
    assert_eq!(dirs[0], Path::new("/cwd/.theway/extensions"));
    assert_eq!(dirs[1], Path::new("/base/extensions"));
}

#[test]
fn discover_sorts_names_and_reads_error_diagnostics_for_unreadable_ts_paths() {
    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    write_extension(
        project.path(),
        "z-last",
        r#"export const kind = "compaction";"#,
    );
    write_extension(
        project.path(),
        "a-first",
        r#"export const kind = "compaction";"#,
    );
    // A directory named `bad.ts` passes the extension filter but can't be
    // read as UTF-8 text; discovery should record a diagnostic and continue.
    let ext_dir = project.path().join(".theway").join("extensions");
    std::fs::create_dir_all(ext_dir.join("bad.ts")).unwrap();

    let registry = ExtensionRegistry::discover(project.path(), user.path());

    assert_eq!(registry.names(), vec!["a-first".to_string(), "z-last".to_string()]);
    assert_eq!(registry.errors.len(), 1);
    assert!(registry.errors[0].contains("bad.ts"), "{:?}", registry.errors);
}

#[test]
fn get_missing_and_by_kind_filter() {
    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    write_extension(
        project.path(),
        "compactor",
        r#"export const kind = "compaction";"#,
    );

    let registry = ExtensionRegistry::discover(project.path(), user.path());

    assert!(registry.get("missing").is_none());
    assert_eq!(registry.by_kind("compaction").len(), 1);
    assert_eq!(registry.by_kind("other").len(), 0);
}

#[test]
fn compact_algorithm_registry_registers_only_compaction_kind() {
    let project = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    write_extension(
        project.path(),
        "compactor",
        r#"export const kind = "compaction";"#,
    );
    let extensions = ExtensionRegistry::discover(project.path(), user.path());

    let registry = compact_algorithm_registry(&extensions);

    assert_eq!(registry.custom_names(), vec!["compactor".to_string()]);
}

#[tokio::test]
async fn decide_compact_falls_back_to_false_on_non_boolean_hook_result() {
    let source = r#"export const kind = "compaction";
export function decide_compact(ctx: any): string { return "yes"; }
"#;
    let ext = Arc::new(TsExtension::new("non-bool".into(), Path::new("/tmp/non-bool.ts").into(), source.into()).unwrap());
    let algorithm = TsCompactAlgorithm::new(ext);
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();

    let result = algorithm.decide_compact(100, 1000, &settings).await;

    assert!(!result, "non-boolean hook result must be treated as decline");
}

#[tokio::test]
async fn select_cut_point_falls_back_to_builtin_when_cut_index_is_invalid() {
    let source = r#"export const kind = "compaction";
export function select_cut_point(ctx: any): any { return { cut_index: "not-a-number" }; }
"#;
    let ext = Arc::new(TsExtension::new("bad-cut".into(), Path::new("/tmp/bad-cut.ts").into(), source.into()).unwrap());
    let algorithm = TsCompactAlgorithm::new(ext);
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();

    let cut = algorithm.select_cut_point(&entries(3), &settings).await;

    // Builtin fallback returns a valid in-bounds result; we only need to
    // observe the fallback path running without panicking.
    assert!(cut.cut_index <= 3);
}

#[tokio::test]
async fn select_cut_point_clamps_out_of_range_cut_index() {
    let source = r#"export const kind = "compaction";
export function select_cut_point(ctx: any): any { return { cut_index: 999 }; }
"#;
    let ext = Arc::new(TsExtension::new("clamp".into(), Path::new("/tmp/clamp.ts").into(), source.into()).unwrap());
    let algorithm = TsCompactAlgorithm::new(ext);
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();

    let cut = algorithm.select_cut_point(&entries(3), &settings).await;

    assert_eq!(cut.cut_index, 3);
    assert_eq!(cut.first_kept_entry_id, None);
}

#[tokio::test]
async fn name_returns_extension_name() {
    let source = r#"export const kind = "compaction";"#;
    let ext = Arc::new(TsExtension::new("named".into(), Path::new("/tmp/named.ts").into(), source.into()).unwrap());
    let algorithm = TsCompactAlgorithm::new(ext);

    assert_eq!(algorithm.name(), "named");
}

#[test]
fn run_hook_returns_none_for_missing_hook() {
    let source = r#"export const kind = "compaction";"#;
    let ext = TsExtension::new("no-hooks".into(), Path::new("/tmp/no-hooks.ts").into(), source.into()).unwrap();

    let out = ext.run_hook("missing_hook", &serde_json::json!({})).unwrap();

    assert_eq!(out, None);
}
