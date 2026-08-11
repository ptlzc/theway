//! `outline` tool — tree-sitter AST-based structural outline. Direct port of the
//! enhanced-tools native addon (`pi-src/extensions/enhanced-tools/native/src/lib.rs`),
//! minus the napi layer; the AST walk rules are verbatim from the addon.
//! Supports TS/TSX/JS/Python/Rust/Go.

#![allow(dead_code)] // exported as a module; wired into default_tools by a later step.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};
use theway_core::{AgentTool, AgentToolError, AgentToolResult, AgentToolUpdate, ToolExecutionMode};
use theway_llm_provider::{Tool, UserContentBlock};
use tokio_util::sync::CancellationToken;
use tree_sitter::Node as TSNode;

pub struct OutlineTool;

#[async_trait]
impl AgentTool for OutlineTool {
    fn definition(&self) -> &Tool {
        &DEFINITION
    }

    fn label(&self) -> &str {
        "outline"
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        Some(ToolExecutionMode::Parallel)
    }

    async fn execute(
        &self,
        _id: &str,
        params: Value,
        cancel: CancellationToken,
        _on_update: Option<AgentToolUpdate>,
    ) -> Result<AgentToolResult, AgentToolError> {
        let path = params
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AgentToolError::from("missing `file_path`"))?;

        if cancel.is_cancelled() {
            return Err(AgentToolError::from("cancelled"));
        }

        // Parse + walk is synchronous CPU work; run in spawn_blocking like grep.
        let path = path.to_string();
        let outline = tokio::task::spawn_blocking(move || outline_file(&path))
            .await
            .map_err(|e| AgentToolError::from(format!("spawn_blocking: {e}")))?
            .map_err(AgentToolError::from)?;

        let entries_len = outline.entries.len();
        let language = outline.language.clone();
        Ok(AgentToolResult {
            content: vec![UserContentBlock::text(outline.render())],
            details: json!({
                "path": outline.file_path,
                "language": language,
                "entries": entries_len,
            }),
            terminate: None,
        })
    }
}

#[derive(Debug)]
struct OutlineEntry {
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
    indent: u32,
}

struct Outline {
    file_path: String,
    language: String,
    entries: Vec<OutlineEntry>,
}

impl Outline {
    /// Output format aligned with `tools/outline.ts`:
    /// `file <path>` / `language <lang>` / blank / `"  ".repeat(indent)kind name [start,end]`.
    fn render(&self) -> String {
        let mut lines = vec![
            format!("file {}", self.file_path),
            format!("language {}", self.language),
            String::new(),
        ];
        for e in &self.entries {
            lines.push(format!(
                "{}{} {} [{},{}]",
                "  ".repeat(e.indent as usize),
                e.kind,
                e.name,
                e.start_line,
                e.end_line
            ));
        }
        if self.entries.is_empty() {
            lines.push("(no structural entries found)".to_string());
        }
        lines.join("\n")
    }
}

fn outline_file(file_path: &str) -> Result<Outline, String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("File not found: {file_path}"));
    }
    if !path.is_file() {
        return Err(format!("Not a file: {file_path}"));
    }

    let (mut parser, lang_name) =
        get_parser(path).ok_or_else(|| format!("Unsupported language for: {file_path}"))?;

    let source = std::fs::read_to_string(path).map_err(|e| format!("Failed to read file: {e}"))?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| "Failed to parse file".to_string())?;

    let mut entries = Vec::new();
    walk_node(tree.root_node(), &source, lang_name, 0, &mut entries);

    Ok(Outline {
        file_path: file_path.to_string(),
        language: lang_name.to_string(),
        entries,
    })
}

/// Detect language from file extension and return the corresponding parser + language name
fn get_parser(path: &Path) -> Option<(tree_sitter::Parser, &'static str)> {
    let ext = path.extension()?.to_str()?;
    let (lang, name): (tree_sitter::Language, &'static str) = match ext {
        "ts" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
        ),
        "tsx" => (tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx"),
        "js" | "jsx" | "mjs" | "cjs" => (tree_sitter_javascript::LANGUAGE.into(), "javascript"),
        "py" => (tree_sitter_python::LANGUAGE.into(), "python"),
        "rs" => (tree_sitter_rust::LANGUAGE.into(), "rust"),
        "go" => (tree_sitter_go::LANGUAGE.into(), "go"),
        _ => return None,
    };

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).ok()?;
    Some((parser, name))
}

/// Extract a name from a node by looking for a child with a specific field name
fn get_name_from_field<'a>(node: TSNode<'a>, source: &'a str, field: &str) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    let start = child.start_byte();
    let end = child.end_byte();
    Some(source[start..end].to_string())
}

/// Get name from the first identifier child (fallback when field names don't work)
fn get_name_from_first_identifier<'a>(node: TSNode<'a>, source: &'a str) -> Option<String> {
    for i in 0..node.named_child_count() {
        if let Some(child) = node.named_child(i) {
            if child.kind() == "identifier" || child.kind() == "type_identifier" {
                let start = child.start_byte();
                let end = child.end_byte();
                return Some(source[start..end].to_string());
            }
        }
    }
    None
}

/// Get the type identifier for a node — tries multiple strategies
fn extract_name(node: TSNode, source: &str) -> Option<String> {
    let kind = node.kind();

    // For variable declarations, the name is the first identifier in variable_declarator
    if kind == "lexical_declaration" || kind == "variable_declaration" {
        // Find variable_declarator
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if child.kind() == "variable_declarator" {
                    // First named child is the identifier
                    if let Some(name_child) = child.named_child(0) {
                        if name_child.kind() == "identifier" {
                            let start = name_child.start_byte();
                            let end = name_child.end_byte();
                            return Some(source[start..end].to_string());
                        }
                    }
                }
            }
        }
        return None;
    }

    // Try field names first
    for field in &["name", "type"] {
        if let Some(name) = get_name_from_field(node, source, field) {
            return Some(name);
        }
    }
    // Fallback: first identifier child
    get_name_from_first_identifier(node, source)
}

/// Determine the outline "kind" for a TS/JS node
fn ts_kind(node: TSNode) -> Option<&'static str> {
    let kind = node.kind();
    let parent_kind = node.parent().map(|p| p.kind());

    match kind {
        "function_declaration" | "generator_function_declaration" => Some("function"),
        "function_expression" | "arrow_function" => {
            // Only include if parent is variable_declarator AND grandparent is top-level
            if parent_kind != Some("variable_declarator") {
                return None;
            }
            // Check grandparent
            let grandparent_kind = node.parent().and_then(|p| p.parent()).map(|gp| gp.kind());
            if matches!(grandparent_kind, Some("program") | Some("export_statement")) {
                Some("function")
            } else {
                None
            }
        }
        "interface_declaration" => Some("interface"),
        "type_alias_declaration" => Some("type"),
        "class_declaration" => Some("class"),
        "method_definition" => Some("method"),
        "lexical_declaration" | "variable_declaration" => {
            // Only include top-level or export-level const declarations
            // (parent is program, export_statement, or class_body)
            // Skip const inside function bodies to avoid noise
            let is_top_level = matches!(
                parent_kind,
                Some("program") | Some("export_statement") | Some("class_body") | Some("module")
            );
            if !is_top_level {
                return None;
            }

            // const X = memo(function X()...) or const X = () => ...
            // Find variable_declarator child, then check its init
            let mut declarator_idx: Option<usize> = None;
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    if child.kind() == "variable_declarator" {
                        declarator_idx = Some(i);
                        break;
                    }
                }
            }
            let declarator = match declarator_idx {
                Some(idx) => node.child(idx).unwrap(),
                None => return None,
            };

            // Check if init is a function/arrow/call (using named_child to skip punctuation)
            for i in 0..declarator.named_child_count() {
                if let Some(nc) = declarator.named_child(i) {
                    let nc_kind = nc.kind();
                    if nc_kind == "arrow_function"
                        || nc_kind == "function_expression"
                        || nc_kind == "call_expression"
                    {
                        return Some("const");
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Determine the outline "kind" for a Python node
fn py_kind(node: TSNode) -> Option<&'static str> {
    match node.kind() {
        "function_definition" => Some("function"),
        "class_definition" => Some("class"),
        _ => None,
    }
}

/// Determine the outline "kind" for a Rust node
fn rs_kind(node: TSNode) -> Option<&'static str> {
    match node.kind() {
        "function_item" => Some("function"),
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "trait_item" => Some("trait"),
        "impl_item" => Some("impl"),
        "type_item" => Some("type"),
        "mod_item" => Some("mod"),
        _ => None,
    }
}

/// Determine the outline "kind" for a Go node
fn go_kind(node: TSNode) -> Option<&'static str> {
    match node.kind() {
        "function_declaration" => Some("function"),
        "method_declaration" => Some("method"),
        "type_spec" => {
            // type_spec is inside type_declaration; its "type" field is the actual type
            // struct_type, interface_type, etc.
            let type_node = node.child_by_field_name("type");
            if let Some(tn) = type_node {
                match tn.kind() {
                    "struct_type" => Some("struct"),
                    "interface_type" => Some("interface"),
                    _ => Some("type"),
                }
            } else {
                Some("type")
            }
        }
        _ => None,
    }
}

/// Recursively walk the AST and collect outline entries
fn walk_node(node: TSNode, source: &str, lang: &str, indent: u32, entries: &mut Vec<OutlineEntry>) {
    // Determine kind based on language
    let kind_fn = match lang {
        "typescript" | "tsx" | "javascript" => ts_kind,
        "python" => py_kind,
        "rust" => rs_kind,
        "go" => go_kind,
        _ => return,
    };

    if let Some(kind) = kind_fn(node) {
        if let Some(name) = extract_name(node, source) {
            // For TS/JS const with function/arrow, check if it's a component
            let final_kind = if (lang == "tsx" || lang == "typescript" || lang == "javascript")
                && kind == "const"
            {
                // Check if name starts with uppercase → component
                if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                {
                    "component"
                } else {
                    "const"
                }
            } else if (lang == "tsx" || lang == "typescript" || lang == "javascript")
                && kind == "function"
            {
                // function declarations with uppercase first letter are components
                if name
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                    && node.kind() == "function_declaration"
                {
                    "component"
                } else {
                    "function"
                }
            } else {
                kind
            };

            entries.push(OutlineEntry {
                kind: final_kind.to_string(),
                name,
                start_line: (node.start_position().row + 1) as u32,
                end_line: (node.end_position().row + 1) as u32,
                indent,
            });

            // For const/component (lexical_declaration), don't recurse into children
            // to avoid duplicate function_expression entries
            if kind == "const" {
                return;
            }
        }
    }

    // Recurse into children using child(i) — children() with cursor is unreliable in 0.25
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            walk_node(child, source, lang, indent + 1, entries);
        }
    }
}

use once_cell::sync::Lazy;
static DEFINITION: Lazy<Tool> = Lazy::new(|| Tool {
    name: "outline".into(),
    description: "Generate a structural outline of a source file (functions, classes, \
                  interfaces, components with line ranges). Uses tree-sitter AST parsing. \
                  Supports: .ts, .tsx, .js, .jsx, .py, .rs, .go. For files >200 lines, use \
                  this first to identify line ranges, then read with offset/limit."
        .to_string(),
    parameters: json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string", "description": "Path to the source file to outline (relative or absolute)" },
        },
        "required": ["file_path"],
    }),
});

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture_path(dir: &tempfile::TempDir, name: &str, content: &str) -> String {
        let p = dir.path().join(name);
        std::fs::write(&p, content).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn rust_real_file_entries_are_nonempty_and_ordered() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/src/agent/run_loop/mod.rs");
        let o = outline_file(path.to_str().unwrap()).unwrap();
        assert_eq!(o.language, "rust");
        assert!(
            !o.entries.is_empty(),
            "agent/run_loop/mod.rs should yield entries"
        );
        assert!(o.entries.iter().any(|e| e.kind == "function"));
        // DFS pre-order over tree-sitter children is source order: start lines
        // non-decreasing, ranges never inverted.
        let mut prev = 0u32;
        for e in &o.entries {
            assert!(e.start_line >= prev, "entry out of source order: {e:?}");
            assert!(e.start_line <= e.end_line, "inverted range: {e:?}");
            prev = e.start_line;
        }
    }

    #[test]
    fn ts_fixture_kinds_and_indent() {
        let dir = tempdir().unwrap();
        let path = fixture_path(
            &dir,
            "sample.tsx",
            r#"import { memo } from "react";

function helper(x: number): number {
  return x * 2;
}

const App = memo(() => {
  return <div>hi</div>;
});

const tick = () => {};

class Foo {
  bar(): void {}
}
"#,
        );
        let o = outline_file(&path).unwrap();
        assert_eq!(o.language, "tsx");
        let text = o.render();
        assert!(text.contains("function helper ["));
        assert!(text.contains("component App ["));
        assert!(text.contains("class Foo ["));
        assert!(text.contains("  method bar ["));
        assert!(text.contains("const tick ["));
    }

    #[test]
    fn py_fixture_kinds_and_indent() {
        let dir = tempdir().unwrap();
        let path = fixture_path(
            &dir,
            "sample.py",
            "import os\n\n\ndef helper(x):\n    return x * 2\n\n\nclass Foo:\n    def bar(self):\n        pass\n",
        );
        let o = outline_file(&path).unwrap();
        assert_eq!(o.language, "python");
        let text = o.render();
        assert!(text.contains("function helper ["));
        assert!(text.contains("class Foo ["));
        assert!(text.contains("  function bar ["));
    }

    #[test]
    fn go_fixture_kinds() {
        let dir = tempdir().unwrap();
        let path = fixture_path(
            &dir,
            "sample.go",
            "package main\n\ntype Person struct {\n    Name string\n}\n\nfunc (p Person) Greet() string {\n    return \"hi\"\n}\n\nfunc main() {}\n",
        );
        let o = outline_file(&path).unwrap();
        assert_eq!(o.language, "go");
        let text = o.render();
        assert!(text.contains("struct Person ["));
        assert!(text.contains("method Greet ["));
        assert!(text.contains("function main ["));
    }

    #[test]
    fn empty_outline_shows_placeholder() {
        let dir = tempdir().unwrap();
        let path = fixture_path(&dir, "empty.py", "# just a comment\n");
        let o = outline_file(&path).unwrap();
        assert!(o.entries.is_empty());
        assert!(o.render().contains("(no structural entries found)"));
    }

    #[tokio::test]
    async fn unknown_extension_is_error() {
        let dir = tempdir().unwrap();
        let path = fixture_path(&dir, "data.txt", "hello\n");
        let tool = OutlineTool;
        let err = tool
            .execute(
                "o",
                json!({ "file_path": path }),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Unsupported language"));
    }

    #[tokio::test]
    async fn missing_file_is_error() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("nope.rs");
        let tool = OutlineTool;
        let err = tool
            .execute(
                "o",
                json!({ "file_path": missing.to_str().unwrap() }),
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("File not found"));
    }
}
