//! TS→JS transpilation (oxc) and QuickJS execution (rquickjs) — the shared engine behind
//! every TS extension. Kept private to `extensions`; the extension contract lives in
//! `mod.rs`.

use std::path::Path;

use serde_json::Value;

/// Transpile a TypeScript module to plain ESM JavaScript. v1 strips types only (plus
/// const-enum / namespace / decorator lowering via the full TS transform); `import`
/// statements are not supported (single-file contract).
pub(super) fn transpile_ts(source: &str, path: &Path) -> Result<String, String> {
    use oxc_allocator::Allocator;
    use oxc_codegen::{Codegen, CodegenOptions};
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    use oxc_transformer::{HelperLoaderMode, TransformOptions, Transformer, TypeScriptOptions};

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path)
        .map_err(|_| format!("unsupported extension: {}", path.display()))?
        .with_typescript(true)
        .with_module(true);
    let ret = Parser::new(&allocator, source, source_type).parse();
    if !ret.errors.is_empty() {
        let diags = ret
            .errors
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("parse error: {diags}"));
    }
    let mut program = ret.program;
    let semantic = SemanticBuilder::new().build(&program);
    let scoping = semantic.semantic.into_scoping();
    let mut options = TransformOptions {
        typescript: TypeScriptOptions::default(),
        ..Default::default()
    };
    // Inline any emitted helpers so the module stays self-contained (no bare imports).
    options.helper_loader.mode = HelperLoaderMode::Inline;
    let ret =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if !ret.errors.is_empty() {
        let diags = ret
            .errors
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("transform error: {diags}"));
    }
    let code = Codegen::new()
        .with_options(CodegenOptions::default())
        .build(&program)
        .code;
    Ok(code)
}

/// Evaluate `js` (ESM) in a fresh QuickJS context and read the `kind` export. Used at
/// discovery time to route an extension to its extension point. `None` when the module
/// has no `kind` export (or evaluation fails).
pub(super) fn read_kind_export(js: &str) -> Option<String> {
    run_module_js(js, |_ctx, namespace| {
        if namespace.contains_key("kind").ok()? {
            namespace.get::<_, String>("kind").ok()
        } else {
            None
        }
    })
    .ok()
    .flatten()
}

/// Evaluate `js` (ESM) in a fresh QuickJS context and invoke one exported hook with the
/// JSON `arg`. The hook result is round-tripped through `JSON.stringify` so no rquickjs
/// value-conversion APIs are needed. `Ok(None)` means the hook is absent or declined.
pub(super) fn run_hook_js(js: &str, hook: &str, arg: &Value) -> Result<Option<Value>, String> {
    let arg_literal = serde_json::to_string(&arg.to_string()).map_err(|e| e.to_string())?;
    // Serialize the hook call: read the hook, JSON.parse the arg, stringify the result
    // (undefined → null so JSON.stringify stays well-formed).
    let script = format!(
        "JSON.stringify((() => {{ \
           const f = globalThis.__theway_ext.{hook}; \
           if (typeof f !== 'function') return null; \
           const r = f(JSON.parse({arg_literal})); \
           return r === undefined ? null : r; \
         }})())"
    );
    let value = run_module_js(js, |ctx, _namespace| {
        let out: String = ctx.eval(script.as_str()).ok()?;
        let value: Value = serde_json::from_str(&out).ok()?;
        if value.is_null() { None } else { Some(value) }
    })?;
    Ok(value)
}

// ──────────────────────────────────────────────────────────────────────────────────────────
// Shared module-evaluation plumbing
// ──────────────────────────────────────────────────────────────────────────────────────────

/// Evaluate ESM `js` in a fresh QuickJS context, expose its namespace as
/// `globalThis.__theway_ext`, then hand `(ctx, namespace)` to `f`. Each call gets a
/// brand-new runtime — cheap, and failures can't leak between calls.
fn run_module_js<T>(
    js: &str,
    f: impl FnOnce(rquickjs::Ctx<'_>, rquickjs::Object<'_>) -> Option<T>,
) -> Result<Option<T>, String> {
    use rquickjs::{Context, Module, Runtime};

    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    let ctx = Context::full(&runtime).map_err(|e| e.to_string())?;
    ctx.with(|ctx| {
        let module =
            Module::declare(ctx.clone(), "theway-extension", js).map_err(|e| e.to_string())?;
        let (module, _promise) = module.eval().map_err(|e| e.to_string())?;
        let namespace = module.namespace().map_err(|e| e.to_string())?;
        ctx.globals()
            .set("__theway_ext", namespace.clone())
            .map_err(|e| e.to_string())?;
        Ok(f(ctx, namespace))
    })
}
