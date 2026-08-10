//! Integration tests for the core-level TS extension system (issue #4): discovery from
//! `.theway/extensions/*.ts`, `kind` routing, hook execution via the embedded QuickJS
//! host, and the compaction-algorithm adapter wired through `CompactAlgorithmRegistry`.

use tempfile::tempdir;
use theway_core::extensions::ExtensionRegistry;
use theway_core::runtime::compaction::algorithm::CompactAlgorithmRegistry;
use theway_core::runtime::compaction::compaction::DEFAULT_COMPACTION_SETTINGS;
use theway_core::runtime::session::session::SessionTreeEntry;
use theway_core::types::AgentMessage;

/// `THEWAY_DIR` is process-global, so discovery tests that point it at a tempdir must be
/// serialized against each other (and against tests that must not see a stray user dir).
static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Discover with a hermetic user dir: `user_dir` (default: a fresh empty tempdir) is
/// installed as `$THEWAY_DIR` for the duration of the call, then removed.
fn discover_with_user_dir(
    cwd: &std::path::Path,
    user_dir: Option<&std::path::Path>,
) -> ExtensionRegistry {
    let _guard = ENV_GUARD.lock().unwrap();
    let owned;
    let dir = match user_dir {
        Some(d) => d,
        None => {
            owned = tempdir().unwrap();
            owned.path()
        }
    };
    unsafe {
        std::env::set_var("THEWAY_DIR", dir);
    }
    let registry = ExtensionRegistry::discover(cwd);
    unsafe {
        std::env::remove_var("THEWAY_DIR");
    }
    registry
}

const FULL_EXT: &str = r#"export const kind = "compaction";
export const description = "test algorithm";

export function decide_compact(ctx: {
  context_tokens: number;
  context_window: number;
  settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number };
}): boolean {
  return ctx.context_tokens > 12345;
}

export function select_cut_point(ctx: {
  entries: unknown[];
  settings: { enabled: boolean; reserve_tokens: number; keep_recent_tokens: number };
}): { cut_index: number } {
  return { cut_index: ctx.entries.length - 2 };
}

export function summarize_prefix(ctx: {
  messages: unknown[];
  custom_instructions?: string;
}): string {
  return "custom summary: " + ctx.messages.length + " messages folded";
}
"#;

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
fn discovers_extension_and_routes_by_kind() {
    let dir = tempdir().unwrap();
    write_extension(dir.path(), "my-algo", FULL_EXT);

    let registry = discover_with_user_dir(dir.path(), None);
    assert!(registry.errors.is_empty(), "{:?}", registry.errors);
    assert_eq!(registry.names(), vec!["my-algo".to_string()]);

    let ext = registry.get("my-algo").expect("extension found");
    assert_eq!(ext.kind(), "compaction");
    assert_eq!(
        registry.by_kind("compaction").len(),
        1,
        "kind routing finds the extension"
    );
    assert!(registry.by_kind("tool").is_empty());
}

#[test]
fn runs_all_hooks_with_json_contract() {
    let dir = tempdir().unwrap();
    write_extension(dir.path(), "my-algo", FULL_EXT);
    let registry = discover_with_user_dir(dir.path(), None);
    let ext = registry.get("my-algo").unwrap();
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();

    // decide_compact: boolean over the numeric context.
    let arg = serde_json::json!({
        "context_tokens": 20000,
        "context_window": 100000,
        "settings": serde_json::to_value(&settings).unwrap(),
    });
    let out = ext.run_hook("decide_compact", &arg).unwrap();
    assert_eq!(out, Some(serde_json::json!(true)));
    let arg = serde_json::json!({
        "context_tokens": 100,
        "context_window": 100000,
        "settings": serde_json::to_value(&settings).unwrap(),
    });
    assert_eq!(
        ext.run_hook("decide_compact", &arg).unwrap(),
        Some(serde_json::json!(false))
    );

    // select_cut_point: entries array → cut_index.
    let arg = serde_json::json!({
        "entries": serde_json::to_value(entries(5)).unwrap(),
        "settings": serde_json::to_value(&settings).unwrap(),
    });
    let out = ext.run_hook("select_cut_point", &arg).unwrap().unwrap();
    assert_eq!(out.get("cut_index").and_then(|c| c.as_u64()), Some(3));

    // summarize_prefix: messages array → literal summary string.
    let arg = serde_json::json!({
        "messages": serde_json::to_value(vec![
            user_message("a"),
            user_message("b"),
            user_message("c"),
        ])
        .unwrap(),
        "settings": serde_json::to_value(&settings).unwrap(),
        "custom_instructions": null,
    });
    let out = ext.run_hook("summarize_prefix", &arg).unwrap();
    assert_eq!(
        out,
        Some(serde_json::json!("custom summary: 3 messages folded"))
    );

    // A hook the file doesn't export → None (caller falls back to builtin).
    assert_eq!(ext.run_hook("nonexistentHook", &arg).unwrap(), None);
}

#[test]
fn registry_resolves_builtin_and_custom_algorithms() {
    let dir = tempdir().unwrap();
    write_extension(dir.path(), "my-algo", FULL_EXT);

    let extensions = discover_with_user_dir(dir.path(), None);
    let registry = CompactAlgorithmRegistry::from_extensions(&extensions);

    assert_eq!(registry.custom_names(), vec!["my-algo".to_string()]);
    // Custom algorithm selected by name.
    assert_eq!(registry.algorithm("my-algo").name(), "my-algo");
    // Builtin + unknown names resolve to the builtin.
    assert_eq!(registry.algorithm("builtin").name(), "builtin");
    assert_eq!(registry.algorithm("").name(), "builtin");
    assert_eq!(registry.algorithm("nope").name(), "builtin");
}

#[tokio::test]
async fn compaction_adapter_dispatches_through_ts_hooks() {
    let dir = tempdir().unwrap();
    write_extension(dir.path(), "my-algo", FULL_EXT);
    let extensions = discover_with_user_dir(dir.path(), None);
    let registry = CompactAlgorithmRegistry::from_extensions(&extensions);
    let algorithm = registry.algorithm("my-algo");
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();

    // decide_compact → TS hook.
    assert!(algorithm.decide_compact(20_000, 100_000, &settings).await);
    assert!(!algorithm.decide_compact(100, 100_000, &settings).await);

    // select_cut_point → TS hook (cut_index = len - 2, first kept id derived host-side).
    let cut = algorithm.select_cut_point(&entries(5), &settings).await;
    assert_eq!(cut.cut_index, 3);
    assert_eq!(cut.first_kept_entry_id.as_deref(), Some("m3"));

    // summarize → TS literal (no LLM call, no network).
    let messages = vec![user_message("a"), user_message("b"), user_message("c")];
    let model = theway_llm_provider::Model {
        id: "faux".into(),
        name: "Faux".into(),
        api: theway_llm_provider::Api::from("faux"),
        provider: theway_llm_provider::Provider::from("faux"),
        base_url: String::new(),
        reasoning: false,
        thinking_level_map: None,
        input: vec![],
        cost: theway_llm_provider::ModelCost::default(),
        context_window: 100_000,
        max_tokens: 0,
        headers: None,
        compat: None,
    };
    let request = theway_core::runtime::compaction::algorithm::SummarizeRequest {
        model: &model,
        messages: &messages,
        custom_instructions: None,
        settings: &settings,
        stream_fn: None,
        cancel: &tokio_util::sync::CancellationToken::new(),
    };
    let out = algorithm.summarize_prefix(&request).await.unwrap();
    assert_eq!(out.summary, "custom summary: 3 messages folded");
}

#[tokio::test]
async fn missing_hooks_fall_back_to_builtin() {
    let dir = tempdir().unwrap();
    write_extension(
        dir.path(),
        "cut-only",
        r#"export const kind = "compaction";
export function select_cut_point(ctx: { entries: unknown[] }): { cut_index: number } {
  return { cut_index: 1 };
}
"#,
    );
    let extensions = discover_with_user_dir(dir.path(), None);
    let registry = CompactAlgorithmRegistry::from_extensions(&extensions);
    let algorithm = registry.algorithm("cut-only");

    // No decide_compact export → builtin 80% heuristic.
    let settings = DEFAULT_COMPACTION_SETTINGS.clone();
    assert!(!algorithm.decide_compact(100, 100_000, &settings).await);
    assert!(algorithm.decide_compact(80_001, 100_000, &settings).await);

    // select_cut_point is custom.
    let cut = algorithm.select_cut_point(&entries(4), &settings).await;
    assert_eq!(cut.cut_index, 1);
}

#[test]
fn invalid_extension_is_skipped_with_diagnostic() {
    let dir = tempdir().unwrap();
    // Syntax error: unclosed brace.
    write_extension(
        dir.path(),
        "broken",
        "export const kind = \"compaction\";\nexport function decide_compact(ctx {",
    );
    // Valid TS but no `kind` export.
    write_extension(dir.path(), "no-kind", "export const helper = 42;\n");

    let registry = discover_with_user_dir(dir.path(), None);
    assert_eq!(registry.names().len(), 0, "both files skipped");
    assert_eq!(registry.errors.len(), 2, "each failure gets a diagnostic");
    assert!(
        registry
            .errors
            .iter()
            .any(|e| e.contains("broken") && e.contains("parse error")),
        "{:?}",
        registry.errors
    );
    assert!(
        registry
            .errors
            .iter()
            .any(|e| e.contains("no-kind") && e.contains("kind")),
        "{:?}",
        registry.errors
    );
}

#[test]
fn project_extension_shadows_user_global() {
    let project = tempdir().unwrap();
    let user = tempdir().unwrap();
    // Same stem in both dirs: project returns true, user returns false.
    write_extension(
        project.path(),
        "shadow",
        r#"export const kind = "compaction";
export function decide_compact(ctx: any): boolean { return true; }
"#,
    );
    let user_ext_dir = user.path().join("extensions");
    std::fs::create_dir_all(&user_ext_dir).unwrap();
    std::fs::write(
        user_ext_dir.join("shadow.ts"),
        r#"export const kind = "compaction";
export function decide_compact(ctx: any): boolean { return false; }
"#,
    )
    .unwrap();

    // The user dir is `$THEWAY_DIR/extensions`; discovery is guarded (see helper).
    let registry = discover_with_user_dir(project.path(), Some(user.path()));

    let ext = registry.get("shadow").expect("extension found");
    assert_eq!(
        ext.run_hook(
            "decide_compact",
            &serde_json::json!({ "context_tokens": 1, "context_window": 2, "settings": {} })
        )
        .unwrap(),
        Some(serde_json::json!(true)),
        "project-local file wins on name collision"
    );
}
