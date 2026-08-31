# Repository Guidelines

## Workspace layout

Rust 2024 Cargo workspace. The root [`Cargo.toml`](Cargo.toml) is the authoritative member list; this section is a lookup aid.

- [`crates/theway-llm-provider`](crates/theway-llm-provider/README.md) — unified streaming LLM client: providers, model/image catalogs, SSE and OAuth helpers.
- [`crates/theway-core`](crates/theway-core/README.md) — daemon runtime core: agent loop and harness, typed runtime sessions, skills loading, compaction, lifecycle hooks, executor seam, and the multiagent DAG engine.
- [`crates/theway-contract`](crates/theway-contract/README.md) — pure leaf contract crate: raw session persistence interfaces, persisted DAG snapshots, session-scoped trigger/cron sidecar models, and the `~/.theway` base-dir/path layout; no workspace dependencies.
- [`crates/theway-storage`](crates/theway-storage/README.md) — SQLite persistence (one `<uuidv7>.db` per session, session archives, DAG run snapshots); depends only on contract among runtime workspace crates, never core or transport.
- [`crates/theway-daemon`](crates/theway-daemon/README.md) — headless `thewayd` daemon: harness assembly, tools, triggers, MCP/LSP wiring, and protocol servers.
- [`crates/theway-tui`](crates/theway-tui/README.md) — the `theway` CLI binary: ratatui client/controller plus offline session commands.
- [`crates/theway-mcp`](crates/theway-mcp/README.md) — MCP client: stdio transport, JSON-RPC framing, tools list/call.
- [`crates/theway-transport`](crates/theway-transport/README.md) — gRPC/web (HTTP+SSE+WS) transport and wire types.
- [`crates/theway-probe`](crates/theway-probe/README.md) — gRPC serviceability probe binary.
- [`crates/theway-markdown-core`](crates/theway-markdown-core/README.md) — headless Markdown parser policy, analysis, statistics, and structural diagnostics.
- [`crates/theway-markdown`](crates/theway-markdown/README.md) — streaming terminal Markdown renderer.
- [`crates/theway-pager-render`](crates/theway-pager-render/README.md) — ratatui pager and feed rendering primitives.
- [`crates/theway-ratatui-textarea`](crates/theway-ratatui-textarea/README.md) — reusable multiline editor and ratatui widget.
- [`crates/mermaid-parser`](crates/mermaid-parser/README.md) — vendored Mermaid parse stage consumed by `dag_plan`'s flowchart adapter.
- [`crates/tests-bridge-macro`](crates/tests-bridge-macro/README.md) — proc macro anchoring mirrored `#[path]` test modules to the crate root.

Layering: `theway-daemon` is the only direct consumer of `theway-core` and composes core, storage, and transport; `theway-tui` depends on transport/storage/contract but never core or daemon; `theway-storage` depends only on `theway-contract` among runtime workspace crates; `theway-transport` never depends on core or storage. Provider-specific code lives under `crates/theway-llm-provider/src/providers/`; daemon tool implementations live under `crates/theway-daemon/src/tools/`. `make layering-check` enforces these edges.

### Daemon positioning

The daemon ([`crates/theway-daemon`](crates/theway-daemon)) is the runtime service for sessions, tools, triggers, and orchestration, facing the protocol layer ([`crates/theway-transport`](crates/theway-transport): gRPC + HTTP/SSE/WS). It has no concept of client form — it does not distinguish TUI, web, headless scripts, or other programs — and carries no UI concepts (colors, layout, keys).

Boundary rules: client-specific appearance and interaction belong to [`crates/theway-tui`](crates/theway-tui); cross-client features define the wire contract first, and the daemon implements only the protocol-side semantics; behavior requiring client coordination is expressed via snapshot fields or events (for example, `runtime_revision` notifies clients to re-read local resources).

## File size governance (>800 lines)

Source and test files stay under ~800 lines; larger files split into a directory (`foo.rs` → `foo/mod.rs` + domain submodules; `tests/<name>/mod.rs` + domain submodule files), splitting by domain, never mechanically. Exceptions:

- [`crates/mermaid-parser/src/parser.rs`](crates/mermaid-parser/src/parser.rs) — extracted from the third-party `mmdr` parser; kept monolithic to stay diff-compatible with upstream extraction. Do not split it.

## Build, test, and lint

The [`Makefile`](Makefile) mirrors `.github/workflows/ci.yml`; prefer it.

- `make check` — type-check the full workspace including tests (`cargo check --workspace --all-targets`).
- `make build` / `make release` — workspace build; release produces `target/release/theway`.
- `make test` — `cargo test --workspace`.
- `make lint` — `cargo clippy --workspace --all-targets -- -D warnings`.
- `make fmt` / `make fmt-check` — rustfmt rewrite / CI check.
- `make package-check` — verify that the extracted `theway-probe` source package builds independently; this dry run does not authorize a crates.io upload.
- `make doc-sync` — verify English/Chinese documentation pairs, structure, and recorded blob hashes.
- `make ci` — the full local pipeline (format, file-size, layering, documentation synchronization, lint, feature-gate, and test checks).
- `make run` / `make install` — run the REPL / install into `~/.cargo/bin`.

## crates.io publication policy

GitHub Release binaries, crates.io packages, and npm SDKs publish together from one `vX.Y.Z` tag through [`.github/workflows/release.yml`](.github/workflows/release.yml). [`scripts/release-validate.sh`](scripts/release-validate.sh) requires the tag, the Cargo workspace version, `sdks/client/package.json`, and `sdks/plugin/package.json` to be the same version; a mismatched tag stops the workflow before any external write. A Cargo workspace member, a package that passes `cargo package` or `cargo publish --dry-run`, and a binary included in CI are not automatically approved for crates.io publication.

- The release issue or release plan must list every approved crates.io `(package, version)` pair before the first upload. The release workflow publishes the fixed dependency-ordered allowlist in [`scripts/release-crates.txt`](scripts/release-crates.txt): `tests-bridge-macro`, `theway-contract`, `theway-mcp`, `theway-llm-provider`, `theway-storage`, `theway-transport`, `theway-core`, `theway-tui`, `theway-daemon`. Any other package requires explicit user approval recorded in the release issue before it is added to that list.
- `theway-probe` is a repository-local gRPC serviceability and release-validation binary. Build and test it from this workspace with `make package-check` or package-specific Cargo commands; do not run `cargo publish -p theway-probe` and do not add it to the crates.io allowlist.
- Never use `cargo publish --workspace`. Publish one approved package at a time with `cargo publish -p <package>`, in dependency order, after confirming the exact version does not already exist. Treat an upload as immutable: after an uncertain response, query crates.io before retrying.
- A request to publish "binaries and packages together" means the GitHub Release binary matrix, the fixed crates.io allowlist, and both npm SDK packages under the same tag, not every binary target or workspace member. Expanding either allowlist requires explicit user confirmation before the external write.
- After the workflow run, verify the exact package versions, repository metadata, and yank state through crates.io. Complete the release only after the installable end-user binaries have also been installed from the registry and their versions checked.

## Testing

Test layout, naming, and structure follow [`docs/rust-test-files.md`](docs/rust-test-files.md) — the single source of truth. Multi-file module suites live in `crates/<name>/tests/<mirrored-src-path>/` and are bridged from the src module by one `#[path]` line (unit-test semantics preserved); inline `mod tests` is only for lightweight unit assertions.

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

- Crate `README.md` and `docs/*.md` files are English defaults paired with sibling `.zh.md` translations and `.i18n.yaml` records; [`docs/i18n/README.md`](docs/i18n/README.md) is the contract. `AGENTS.md` files remain English-only. Update both sides in one change, run `scripts/verify-doc-i18n.py --write <source.md>`, then run `make doc-sync`.
- Documents state the current mechanism, not change history. Avoid "previously/now/no longer", PR/commit references, and migration narration in durable prose; change stories belong in commit messages or memory notes.
- Cross-reference repository files with relative Markdown paths, never bare filenames.
- Links in a crate's `README.md`, `docs/*.md`, and `AGENTS.md` must resolve inside that crate; describe workspace-root and sibling-crate concepts without repository links so every crate documentation set remains self-contained.
- One physical line per paragraph (editor soft-wrap). Code blocks, tables, and list structure keep their own formatting.
- Name the exact mechanism — function, file path, command, flag — not metaphorical "gate", "surface", or "vocabulary".
- No implementation-status annotations in prose or diagrams ("implemented!", "future: …"); the repo layout and manifests carry status. Unimplemented work is marked `TODO` in code with what is missing, never "deliberate" or "by design".
- No reasoning transcripts in comments or docs: keep the resulting contract (behavior, failure modes, timing, ownership, exceptions), delete narration, test walkthroughs, and rejected alternatives.
- State each rule in one home and link to it from the rest.
- Reserve emphasis for the clause that changes behavior.
