# Repository Guidelines

## Workspace layout

Rust 2024 Cargo workspace. The root [`Cargo.toml`](Cargo.toml) is the authoritative member list; this section is a lookup aid.

- `crates/theway-llm-provider` — unified streaming LLM client: providers, model/image catalogs, SSE and OAuth helpers.
- `crates/theway-core` — agent runtime: harness, session storage, skills loading, compaction, lifecycle hooks, and the multiagent DAG engine.
- `crates/theway-storage` — SQLite persistence (session repos, DAG run snapshots).
- `crates/theway-daemon` — headless `thewayd` daemon (bin `src/bin/thewayd.rs`): harness assembly, tools, triggers, MCP/LSP wiring.
- `crates/theway-tui` — the `theway` CLI binary: ratatui REPL plus local session commands.
- `crates/theway-mcp` — MCP client: stdio transport, JSON-RPC framing, tools list/call.
- `crates/theway-transport` — gRPC/web (HTTP+SSE+WS) transport and wire types.
- `crates/mermaid-parser` — vendored mermaid parse stage consumed by `dag_plan`'s flowchart subset.
- `crates/theway-probe` — gRPC serviceability probe binary (`theway --grpc`).
- `crates/tests-bridge-macro` — proc macro anchoring mirrored `#[path]` test modules to the crate root.

Layering: daemon/tui depend on core/storage/transport; everything sits on `theway-llm-provider`. Provider-specific code lives under `crates/theway-llm-provider/src/providers/`; daemon tool implementations under `crates/theway-daemon/src/tools/`.

### Daemon positioning

The daemon ([`crates/theway-daemon`](crates/theway-daemon)) is the runtime service for sessions, tools, triggers, and orchestration, facing the protocol layer ([`crates/theway-transport`](crates/theway-transport): gRPC + HTTP/SSE/WS). It has no concept of client form — it does not distinguish TUI, web, headless scripts, or other programs — and carries no UI concepts (colors, layout, keys).

Boundary rules: client-specific appearance and interaction belong to [`crates/theway-tui`](crates/theway-tui); cross-client features define the wire contract first, and the daemon implements only the protocol-side semantics; behavior requiring client coordination is expressed via snapshot fields or events (for example, `runtime_revision` notifies clients to re-read local resources).

## File size governance (>800 lines)

Source and test files stay under ~800 lines; larger files split into a directory (`foo.rs` → `foo/mod.rs` + domain submodules; `tests/<name>/mod.rs` + domain submodule files), splitting by domain, never mechanically. Exceptions:

- [`crates/mermaid-parser/src/parser.rs`](crates/mermaid-parser/src/parser.rs) — extracted from the third-party `mmdr` parser; kept monolithic to stay diff-compatible with upstream extraction. Do not split it.
- [`crates/theway-core/src/agent/assembly.rs`](crates/theway-core/src/agent/assembly.rs) — the `AgentHarness` composer (Agent + Session + skills + compaction + permission + lifecycle); kept monolithic so the composed agent API reads as one unit. Do not split it.

## Build, test, and lint

The [`Makefile`](Makefile) mirrors `.github/workflows/ci.yml`; prefer it.

- `make check` — type-check the full workspace including tests (`cargo check --workspace --all-targets`).
- `make build` / `make release` — workspace build; release produces `target/release/theway`.
- `make test` — `cargo test --workspace`.
- `make lint` — `cargo clippy --workspace --all-targets -- -D warnings`.
- `make fmt` / `make fmt-check` — rustfmt rewrite / CI check.
- `make ci` — the full local pipeline (fmt-check + lint + test).
- `make run` / `make install` — run the REPL / install into `~/.cargo/bin`.

## Testing

Test layout, naming, and structure follow [`docs/RUST_TEST_FILES.md`](docs/RUST_TEST_FILES.md) — the single source of truth. Multi-file module suites live in `crates/<name>/tests/<mirrored-src-path>/` and are bridged from the src module by one `#[path]` line (unit-test semantics preserved); inline `mod tests` is only for lightweight unit assertions.

Tests never hit real provider APIs. CI clears provider API-key environment variables; a test that requires a live call must be explicitly gated.

## Git workflow

- Issue-first: create `gh issue create` when a task lands, reference the issue in commit messages (`feat(#12): …`), close it (`gh issue close`) when done. No issue, no implementation.
- Push to `main` directly after committing; use Conventional Commits with the issue reference.
- Never modify, drop, or revert another agent's uncommitted changes (`git checkout --` / `git restore` / `git reset --hard` are banned).
- Commit by ownership (core / daemon / docs as separate commits); stage files explicitly — never mix `.pi/` state or `.agents/` notes in with `git add -A`.
- Parallel subtasks run in worktrees managed by [`scripts/wt.sh`](scripts/wt.sh): `wt start <title>` (issue + worktree + branch), `wt push`, `wt mr`, `wt merge --rm-wt`, `wt cleanup`. Sync each finished step to `main`; remove the worktree when done.
- Before any `git checkout` / `git merge`: `git status --porcelain` and commit/stash uncommitted work first.

## Security & configuration

Do not commit API keys or local session data. Provider keys come from environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and related provider-specific keys). Runtime state lives under `~/.theway/` by default, or `$THEWAY_DIR` when set.

## Subagent orchestration (DAG / subagent tool)

Operating rules for any orchestrator driving the built-in subagents (`subagent` tool, `dag_*` tools). The built-in specs live in [`crates/theway-daemon/src/agent_specs.rs`](crates/theway-daemon/src/agent_specs.rs).

1. Multi-directory tasks pin the target directory. The task prompt starts with `cd <absolute target>`, forbids touching the orchestrator's cwd, and names the files it may modify. The orchestrator supplies the concrete path; subagents cannot guess it.
2. Stalled nodes are the orchestrator's to handle. A node's own idle timeout (default 120s, per-node `timeout` override) fails a stalled node; the orchestrator may instead `dag_skip` it (downstream treats it as done) and take over the remaining work. Verify whatever a stalled node produced.
3. Only the final publish node writes version control. Subagents run `git add/commit/push` only when the task explicitly authorizes it; the orchestrator owns commits, and a `publish`-style node at the end of the DAG is the single allowed writer.
4. DAG node task text is the contract. Every non-root node declares `dependsOn` (a missing `dependsOn` runs everything in parallel). Task text carries absolute paths, forbidden operations, and the acceptance check — the node label is not the task.

## Documentation standards

- Documents state the current mechanism, not change history. Avoid "previously/now/no longer", PR/commit references, and migration narration in durable prose; change stories belong in commit messages or memory notes.
- Cross-reference repository files with relative Markdown paths, never bare filenames.
- One physical line per paragraph (editor soft-wrap). Code blocks, tables, and list structure keep their own formatting.
- Name the exact mechanism — function, file path, command, flag — not metaphorical "gate", "surface", or "vocabulary".
- No implementation-status annotations in prose or diagrams ("implemented!", "future: …"); the repo layout and manifests carry status. Unimplemented work is marked `TODO` in code with what is missing, never "deliberate" or "by design".
- No reasoning transcripts in comments or docs: keep the resulting contract (behavior, failure modes, timing, ownership, exceptions), delete narration, test walkthroughs, and rejected alternatives.
- State each rule in one home and link to it from the rest.
- Reserve emphasis for the clause that changes behavior.
