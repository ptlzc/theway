# Slash-command system + completion

> Parent: master roadmap issue.
> Tier: 1 (daily UX).

## Goal

Today the REPL only recognises four hard-coded strings (`/help`, `/clear`, `/quit`, `/exit`).
Mature agents bind every common operation to a slash command and offer tab completion.

Ship:

- A registry of slash commands with handler closures (`fn(&mut Repl, args) -> Action`).
- Tab completion on the input prefix → list of commands + per-command argument completions
  (e.g. `/model <provider>:<id>` completes against the model catalog).
- Built-in commands: `/model`, `/thinking`, `/compact`, `/diff`, `/undo`, `/diag`, `/save`,
  `/share`, `/help [cmd]`, `/clear`, `/quit`, `/sessions`, `/cwd`, `/skills`, `/tools`.
- Discoverability: `/` alone opens an inline completion menu listing every command + one-line
  description.

## Architecture

```
theway/src/commands/
  mod.rs        SlashCommand trait + Registry
  registry.rs   built-in registration; iter over registered commands
  parse.rs      "/cmd arg1 arg2" → (name, argv)
  complete.rs   PrefixCompleter — returns candidates for input.starts_with('/')
  builtins/     one file per command
```

Each command is:

```rust
trait SlashCommand {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn complete(&self, argv: &[&str], ctx: &CompletionCtx) -> Vec<Candidate> { vec![] }
    async fn run(&self, argv: &[String], ctx: &mut CommandCtx) -> CommandOutcome;
}
```

`CommandOutcome` is small: `Handled`, `Aborted`, `Quit`, `Reload`, `Error(String)`.

Commands that mutate agent state (e.g. `/model`) go through `AgentHarness` setters, never poke
`Agent::state()` directly.

## Stability

- One command runs at a time; the registry takes an exclusive lock on REPL state for the
  duration. The REPL spinner reflects "running command".
- `/undo` / `/compact` / model-switch operations must be transactional w.r.t. the session
  jsonl — if append fails, in-memory state rolls back. Wrap in a `try_with_rollback` helper.
- Commands that talk to external services (`/share` → gist, `/diag` → maybe a metrics dump)
  have a 10s timeout and a clear error path.

## Extensibility

- Third-party extensions (Tier 4) register additional commands via
  `Registry::register(cmd: Arc<dyn SlashCommand>)`.
- Per-project overrides via `.theway/commands/` (TS parity); files are markdown templates that
  expand inline (template = command body), or `.toml` declarative descriptors that bind a
  template + arg schema. Decision required at design time; default to markdown templates only,
  hosted by a builtin `TemplateCommand` runner.

## Performance

- Completion runs on every keystroke; must be O(n) in registered commands (≤ 30) with
  inexpensive `starts_with` checks. No file IO unless the command explicitly opted in (e.g.
  `/sessions` completion lists files but it's behind a debounce).
- `/compact` already exists in the agent core; this issue just wires the trigger.

## Testing

| Layer | What |
|---|---|
| **unit** | Parser splits quoted args; registry dispatch by name; `/model` candidate generation against an in-memory catalog. |
| **integration** | `/model` switch updates session jsonl model-change row; `/undo` pops the last assistant message + writes the rewind row; `/compact` produces a compaction entry. |
| **e2e** | PTY-driven: type `/`, expect completion popup; type `/model `+TAB, expect candidate list; `/quit` exits. `/share` is mocked out (no real gist creation in CI). |

## Acceptance criteria

- Typing `/` and `TAB` lists every registered command.
- `/model anthropic:claude-3-5-sonnet-latest` switches the active model and the next turn is
  served by that model.
- `/undo` removes the most recent assistant turn from the active leaf AND from session jsonl
  state (verifiable by re-opening).
- `/help <cmd>` prints the per-command help block (description + usage + examples).
- Unknown `/foo` prints "unknown command — try `/help`" without crashing the REPL.

## Out of scope

- Custom user-defined commands stored as project markdown templates — design covered here but
  ship as a follow-up under [[09-extensions-skills]].
- `/share` Gist upload — design covered here but ship gated behind [[05-continue-named-sessions]]
  which lands the HTML export pipeline first.
