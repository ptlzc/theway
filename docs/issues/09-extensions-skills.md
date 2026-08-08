# Skills loader (first-class in `AgentHarness`) + extension/plugin system

> Parent: master roadmap issue.
> Tier: 4 (framework depth).
>
> **Priority (2026-05-20)**: per maintainer direction, the **skills** half of this issue is a
> top-of-stack item — built into `theway-core::AgentHarness` rather than an opt-in
> extension. The **extension/plugin system** half remains in scope but lands second.

## Goal

Two related deliverables packaged together because they share the resource-loader
infrastructure. They are split in priority:

### Part A — Skills loader (priority)

Markdown files in `~/.theway/skills/` and `.theway/skills/` with YAML frontmatter (`name`,
`description`, `when`). Skills are an **in-tree feature of `theway-core::AgentHarness`**:

- Loaded at session start, with project-local layer overriding user-global with the same name.
- "Always-on" skills with no `when` clause are concatenated into the system prompt at startup.
- Conditional skills (`when:` regex / glob match against user input) activate on matching turn
  and remain attached for the rest of the session.
- Explicit `/skill <name>` slash command attaches a skill on demand.
- `/skills` lists loaded + active.

Skills are the canonical mechanism for "teach theway how to do X in this repo" — they're cheap to
ship, they survive `--resume`, and they don't require any extension runtime.

### Part B — Extension/plugin system

Load Rust- or WASM-compiled plugins from `~/.theway/extensions/` and `.theway/extensions/` that can:

- register tools,
- subscribe to lifecycle events (`session_start`, `tool_call`, `agent_end`),
- register slash commands,
- contribute message renderers / autocomplete providers.

Lower priority than Part A: the WASM host has a bigger blast radius and a longer design loop.
Ship Part A first; treat Part B as a follow-on PR.

## Architecture

```
theway-core/src/harness/skills/        Part A — first-class skills
  mod.rs        SkillSet — load, match, attach
  loader.rs     three-layer precedence: .theway/skills/ > ~/.theway/skills/ > builtin
  activation.rs SkillActivation = AlwaysOn | WhenMatches(RegexSet)
  prompt.rs     compose_system_prompt(base, skills) — stable ordering

theway/src/loader/extensions.rs Part B — extension hosts
  wasm.rs       wasmtime + WASI preview2 imports
  dylib.rs      libloading + extern "C" entry, behind a feature flag
  abi.rs        stable ABI surface (versioned)
```

`AgentHarness` gains a `skills: SkillSet` field. `AgentHarnessOptions` gains
`skill_roots: Vec<PathBuf>` (default: `~/.theway/skills` + `<project>/.theway/skills`). System-prompt
rendering inside the harness composes base prompt + always-on skills; the `prepare_next_turn`
hook (already exists) attaches matching conditional skills before sending.

Extensions register via `Registry::register(Arc<dyn Extension>)`. The WASM host is `wasmtime`
with capability-controlled imports (fs scope, agent-state read-only, tool registration, log).
A panic in either path is caught and the offender disabled for the session.

## Stability

- Skill frontmatter parsing tolerates malformed files (skip + log).
- Same-name skill collision resolves by precedence (project > user > builtin) and logs a
  warning.
- `when` regex set is bounded — DFA size cap; failure to compile invalidates the skill, not the
  whole load.
- Extension panics cannot kill the agent (`catch_unwind` for dylibs, wasmtime traps for WASM).
- Resource conflicts (two extensions register the same slash command) resolve by precedence
  and log a warning.

## Extensibility

- Skill format is markdown-with-frontmatter; future fields (e.g. `tools_required`,
  `applies_to_models`) extend the schema without breaking older files.
- ABI is the public extension contract — versioned with semver, breaking changes only on minor
  bumps (lockstep with theway itself).
- Extensions can request additional WASM imports via the host policy file
  `~/.theway/extensions.toml`.

## Performance

- Skill activation regex set is compiled once at load and shared across the session.
- Always-on skills are stitched into the system prompt at session start — zero cost per turn.
- Extensions load on session start, off the hot path.

## Testing

| Layer | What |
|---|---|
| **unit** | Three-layer resolver picks project > user > builtin; frontmatter parser handles missing/bad fields; `when` matcher activates on the right inputs; system-prompt composer is deterministic. |
| **integration** | An always-on skill appears in `Agent::state().messages[0]` system prompt; a `when:` skill activates exactly once on matching turn and stays attached; `/skills` reflects status. ABI version negotiation passes/fails as designed (Part B). |
| **e2e** | (Part A) `theway` started in a tempdir with `~/.theway/skills/foo.md` and `<cwd>/.theway/skills/foo.md` containing different bodies; project wins; assistant request body (captured via faux provider) contains project's body. (Part B) trivial wasm extension registers a tool; full e2e turn confirms invocation + result. |

## Acceptance criteria

### Part A — skills

- `~/.theway/skills/foo.md` with `always: true` (or empty `when`) is included in the system prompt
  of every session.
- Conditional skill activates on a matching prompt regex and remains visible to the LLM on
  subsequent turns of the same session.
- Project-local `.theway/skills/foo.md` overrides user-global with same name.
- `/skills` lists loaded skills with their status (always-on / active / inactive).
- `--resume` rebuilds the active set deterministically from the session jsonl.

### Part B — extensions

- A WASM extension installed under `~/.theway/extensions/` registers a tool that appears in
  `/tools` and is invocable by the LLM.
- Extension panic does NOT crash the agent.

## Out of scope

- Marketplace UI for discovering skills/extensions.
- Sandbox escape audit of WASM imports — defer to a security review milestone.
- Hot-reload of skills or extensions mid-session.
- Skill format conversion (e.g. importing Claude Skills exactly as-is) — future enhancement.
