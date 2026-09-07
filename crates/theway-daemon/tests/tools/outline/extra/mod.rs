//! Additional tests for `outline` — kept in a separate bridged module so the
//! original inline suite stays untouched (see docs/rust-test-files.md).

use super::super::*;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use theway_core::ToolExecutionMode;
use theway_core::executor::{CommandOutput, ExecutorError, ExecutorKind, ToolExecutor};

fn local_exec() -> Arc<dyn ToolExecutor> {
    Arc::new(crate::executor::local::LocalExecutor::new())
}

struct FailingReadExecutor;

#[async_trait::async_trait]
impl ToolExecutor for FailingReadExecutor {
    async fn kind(&self) -> ExecutorKind {
        ExecutorKind::Local
    }

    async fn read_file(&self, _path: &Path) -> Result<String, ExecutorError> {
        Err(ExecutorError::Other("boom".into()))
    }

    async fn write_file(&self, _path: &Path, _content: &str) -> Result<(), ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn run_command(
        &self,
        _cwd: &Path,
        _argv: &[String],
        _timeout: Duration,
    ) -> Result<CommandOutput, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn list_dir(&self, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn grep(&self, _pattern: &str, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn find(&self, _glob: &str, _path: &Path) -> Result<Vec<String>, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }

    async fn git(&self, _args: &[String]) -> Result<CommandOutput, ExecutorError> {
        Err(ExecutorError::UnsupportedKind(ExecutorKind::Local))
    }
}

#[test]
fn get_parser_supports_every_documented_extension() {
    for (ext, lang) in [
        ("ts", "typescript"),
        ("tsx", "tsx"),
        ("js", "javascript"),
        ("jsx", "javascript"),
        ("mjs", "javascript"),
        ("cjs", "javascript"),
        ("py", "python"),
        ("rs", "rust"),
        ("go", "go"),
    ] {
        let file_name = format!("sample.{ext}");
        let path = Path::new(&file_name);
        let (_, name) = get_parser(path).unwrap_or_else(|| panic!("expected parser for .{ext}"));
        assert_eq!(name, lang, "unexpected language for .{ext}");
    }
}

#[test]
fn get_parser_rejects_unknown_or_missing_extension() {
    for path in ["Makefile", "x.txt", "x.unknown"] {
        assert!(
            get_parser(Path::new(path)).is_none(),
            "expected no parser for {path}"
        );
    }
}

#[test]
fn outline_from_source_ts_covers_interface_type_alias_and_generator() {
    let (parser, lang) = get_parser(Path::new("sample.ts")).unwrap();
    let source =
        "interface Point { x: number }\ntype Alias = string;\nfunction* gen() { yield 1; }\n";
    let outline = outline_from_source("sample.ts".into(), parser, lang, source.into()).unwrap();

    let text = outline.render();
    assert!(text.contains("interface Point ["), "got: {text}");
    assert!(text.contains("type Alias ["), "got: {text}");
    assert!(text.contains("function gen ["), "got: {text}");
}

#[test]
fn outline_from_source_rust_covers_all_declaration_kinds() {
    let (parser, lang) = get_parser(Path::new("sample.rs")).unwrap();
    let source = "\
struct S;
enum E { A }
trait T {}
impl S {}
type Alias = S;
mod m {}
fn f() {}
";
    let outline = outline_from_source("sample.rs".into(), parser, lang, source.into()).unwrap();

    let text = outline.render();
    for expected in [
        "struct S [",
        "enum E [",
        "trait T [",
        "impl S [",
        "type Alias [",
        "mod m [",
        "function f [",
    ] {
        assert!(text.contains(expected), "missing {expected} in:\n{text}");
    }
}

#[test]
fn outline_from_source_go_covers_type_alias_and_interface() {
    let (parser, lang) = get_parser(Path::new("sample.go")).unwrap();
    let source = "\
package main
type MyString string
type Person struct { Name string }
type Greeter interface { Greet() }
func main() {}
";
    let outline = outline_from_source("sample.go".into(), parser, lang, source.into()).unwrap();

    let text = outline.render();
    for expected in [
        "type MyString [",
        "struct Person [",
        "interface Greeter [",
        "function main [",
    ] {
        assert!(text.contains(expected), "missing {expected} in:\n{text}");
    }
}

#[test]
fn definition_label_and_execution_mode_are_registered() {
    let tool = OutlineTool::new(local_exec());

    assert_eq!(tool.definition().name, "outline");
    assert_eq!(tool.label(), "outline");
    assert_eq!(tool.execution_mode(), Some(ToolExecutionMode::Parallel));
    assert_eq!(tool.definition().parameters["required"][0], "file_path");
}

#[tokio::test]
async fn execute_missing_file_path_is_error() {
    let tool = OutlineTool::new(local_exec());

    let err = tool
        .execute("o", serde_json::json!({}), CancellationToken::new(), None)
        .await
        .expect_err("missing file_path must fail");

    assert!(
        err.to_string().contains("missing `file_path`"),
        "got: {err}"
    );
}

#[tokio::test]
async fn execute_cancelled_before_stat_returns_cancelled() {
    let tool = OutlineTool::new(local_exec());
    let cancel = CancellationToken::new();
    cancel.cancel();

    let err = tool
        .execute(
            "o",
            serde_json::json!({ "file_path": "sample.rs" }),
            cancel,
            None,
        )
        .await
        .expect_err("cancelled execute must fail");

    assert_eq!(err.to_string(), "cancelled");
}

#[tokio::test]
async fn execute_directory_is_not_a_file_error() {
    let dir = tempfile::tempdir().unwrap();
    let tool = OutlineTool::new(local_exec());

    let err = tool
        .execute(
            "o",
            serde_json::json!({ "file_path": dir.path().to_str().unwrap() }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("directory must fail");

    assert!(err.to_string().contains("Not a file"), "got: {err}");
}

#[tokio::test]
async fn execute_read_error_maps_with_context() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    std::fs::write(&path, "fn f() {}\n").unwrap();
    let tool = OutlineTool::new(Arc::new(FailingReadExecutor));

    let err = tool
        .execute(
            "o",
            serde_json::json!({ "file_path": path.to_str().unwrap() }),
            CancellationToken::new(),
            None,
        )
        .await
        .expect_err("read failure must surface");

    let msg = err.to_string();
    assert!(
        msg.contains("Failed to read file:") && msg.contains("boom"),
        "got: {msg}"
    );
}

#[test]
fn ts_kind_covers_export_statement_variable_and_uppercase_function() {
    let (parser, lang) = get_parser(Path::new("sample.tsx")).unwrap();
    let source = "\
export const Exported = () => {};
export function App() { return null; }
function lower() {}
";
    let outline = outline_from_source("sample.tsx".into(), parser, lang, source.into()).unwrap();
    let text = outline.render();
    assert!(text.contains("component Exported ["), "got: {text}");
    assert!(text.contains("component App ["), "got: {text}");
    assert!(text.contains("function lower ["), "got: {text}");
}

#[test]
fn ts_kind_covers_var_function_expression_and_nested_const_skipped() {
    let (parser, lang) = get_parser(Path::new("sample.js")).unwrap();
    let source = "\
var old = function oldStyle() {};
function outer() {
  const hidden = () => {};
}
";
    let outline = outline_from_source("sample.js".into(), parser, lang, source.into()).unwrap();
    let text = outline.render();
    assert!(text.contains("const old ["), "got: {text}");
    assert!(text.contains("function outer ["), "got: {text}");
    assert!(!text.contains("hidden"), "nested const must be skipped: {text}");
}

#[test]
fn walk_node_unknown_language_is_a_noop() {
    let mut entries = Vec::new();
    let (mut parser, _lang) = get_parser(Path::new("sample.rs")).unwrap();
    let tree = parser.parse("fn f() {}", None).expect("parse");
    walk_node(tree.root_node(), "fn f() {}", "cobol", 0, &mut entries);
    assert!(entries.is_empty());
}

#[test]
fn go_type_spec_interface_and_non_struct_types_are_distinguished() {
    let (parser, lang) = get_parser(Path::new("sample.go")).unwrap();
    let source = "\
package main
type A string
type B struct { X int }
type C interface { M() }
";
    let outline = outline_from_source("sample.go".into(), parser, lang, source.into()).unwrap();
    let text = outline.render();
    assert!(text.contains("type A ["), "got: {text}");
    assert!(text.contains("struct B ["), "got: {text}");
    assert!(text.contains("interface C ["), "got: {text}");
}

#[test]
fn plain_const_without_function_init_is_not_outlined() {
    let (parser, lang) = get_parser(Path::new("sample.js")).unwrap();
    let source = "const plain = 1;\n";
    let outline = outline_from_source("sample.js".into(), parser, lang, source.into()).unwrap();
    let text = outline.render();
    assert!(!text.contains("plain"), "got: {text}");
}

#[test]
fn function_expression_not_in_variable_declarator_is_skipped() {
    let (parser, lang) = get_parser(Path::new("sample.js")).unwrap();
    // `bar` appears as a function expression argument, not as a top-level variable
    // initializer, so ts_kind must reject it before it can be outlined.
    let source = "foo(function bar() { return 1; });\n";
    let outline = outline_from_source("sample.js".into(), parser, lang, source.into()).unwrap();
    let text = outline.render();
    assert!(!text.contains("bar"), "got: {text}");
}
