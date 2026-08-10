# Custom compaction algorithms via TS extensions (issue #4)

`theway-core` exposes a **core-level TS extension system** (`crate::extensions`): users
drop single-file TypeScript extensions into `.theway/extensions/`, and the runtime
discovers, transpiles (oxc) and executes them in an embedded QuickJS context — no external
runtime, no npm, no build step. Any capability can declare an extension point; the first
one is **custom compaction algorithms** (`kind = "compaction"`), alongside the built-in
default.

## Why not the existing plugin system?

The pre-existing plugin surface (`crates/server/src/extensions.rs`, issue #9 Part B) is a
Rust-native, compile-time trait registry (`AgentExtension`) — it cannot load pi-style TS
extensions (pi's format is TS ESM loaded by pi's own runtime, tied to `pi-tui` /
`pi-coding-agent` internals), and it has no JS runtime behind it. The extension system here
is theway's own convention: **inspired by pi's directory discovery, but a distinct,
self-contained contract.**

## Discovery

```
<cwd>/.theway/extensions/*.ts     # project-local (wins on name collision)
$THEWAY_DIR/extensions/*.ts       # user-global (default ~/.theway/extensions/)
```

- The file stem is the extension name (and, for compaction, the algorithm name).
- Files without a valid `kind` export (or with TS syntax errors) are skipped with a
  diagnostic in `ExtensionRegistry::errors` — never fatal.
- Select a compaction algorithm via `CompactionSettings.algorithm`
  (default `"builtin"`); unknown names fall back to the builtin with a warning.

## Contract (one file = one extension)

```ts
// ~/.theway/extensions/my-algo.ts
export const kind = "compaction"; // which extension point this file implements

// All hooks are optional; a missing hook falls back to the builtin behavior for that step.
// A hook may also return undefined/null to decline (same fallback).

export function shouldCompact(ctx: {
  contextTokens: number;
  contextWindow: number;
  settings: { enabled: boolean; reserveTokens: number; keepRecentTokens: number; algorithm: string };
}): boolean | undefined { ... }

export function findCutPoint(ctx: {
  entries: unknown[]; // serialized SessionTreeEntry[] (serde tags, camelCase fields)
  settings: { enabled: boolean; reserveTokens: number; keepRecentTokens: number; algorithm: string };
}): { cutIndex: number } | undefined { ... } // entries[..cutIndex] will be folded

export function summarize(ctx: {
  messages: unknown[]; // serialized AgentMessage[]
  settings: { enabled: boolean; reserveTokens: number; keepRecentTokens: number; algorithm: string };
  customInstructions?: string;
}): string | undefined { ... } // literal summary text (no LLM call from TS in v1)
```

## Built-in default (`"builtin"`)

The shipped algorithm needs no TS file: 80%-of-window trigger heuristic +
turn-boundary-safe `keep_recent_tokens` cut + LLM summarization (with the overflow-budget
retry loop). It is implemented in Rust
(`crates/core/src/runtime/compaction/algorithm.rs::BuiltinCompactAlgorithm`) and doubles as
the fallback for every hook a TS extension doesn't override.

## Architecture

```
crates/core/src/extensions/        core-level TS extension system
  mod.rs     TsExtension + ExtensionRegistry (discovery, kind routing, contract docs)
  ts.rs      oxc TS→JS transpile + rquickjs (embedded QuickJS) hook execution
crates/core/src/runtime/compaction/
  algorithm.rs   CompactAlgorithm trait + BuiltinCompactAlgorithm + registry
                 (TsCompactAlgorithm adapter maps a kind="compaction" extension to the trait)
  compaction.rs  compact() orchestration — dispatches through the trait
```

- Each hook call gets a **fresh QuickJS context** (cheap, isolated; a throwing extension
  can't corrupt the host). JSON in, JSON out — hooks are pure functions over the context.
- `ts-extensions` is a cargo feature (on by default) so embedders that only want the bare
  agent (e.g. wasm) don't pay for oxc/rquickjs.

## v1 limitations

- Single-file extensions: no `import`/`export` of other modules, no npm deps.
- Hooks are synchronous pure functions: no async hooks, no host callbacks (LLM calls from
  TS are a future extension).
- `summarize` returns a literal string; LLM-backed summarization stays builtin-only.
- Discovery happens at harness startup (no hot reload).

## Testing

- `crates/core/tests/ts_extensions.rs` — discovery, kind routing, JSON hook contract,
  project>user shadowing, invalid-file diagnostics, and the full
  `CompactAlgorithmRegistry` dispatch (custom hooks + builtin fallback).
