# Subagent / Task delegation

> Parent: master roadmap issue.
> Tier: 4 (framework depth).

## Goal

The TS `pi` (and Claude Code) expose a `Task`/`Agent` tool that lets the main agent delegate a
self-contained sub-investigation to a fresh agent — own model, own tool subset, own context
budget. The result returns as a single tool result. Benefits:

- Protects the parent context from huge file dumps.
- Allows parallel research (multiple subagents fired concurrently).
- Allows lighter / cheaper models for narrow tasks (Haiku-tier for code search).

Ship:

- `Task` tool registered by default in `theway`.
- Per-subagent-type configuration: model, system prompt prefix, tool subset, max-iterations.
- Concurrent subagent execution with a configurable cap.
- Result aggregation into a single text tool result returned to the parent.

## Architecture

```
theway-core/src/subagent/
  spawn.rs        spawn_subagent(SubagentSpec, prompt) -> JoinHandle<Result>
  spec.rs         SubagentSpec { id, description, model, tools, max_iters, system_prompt }
  registry.rs     SubagentRegistry — built-in specs + extension-contributed
```

`Task` tool:

- args: `{ subagent_type: String, description: String, prompt: String }` (+ optional
  `parallel_group` for batching).
- behavior: looks up the spec by `subagent_type`, spawns a fresh `AgentHarness` with a *new*
  in-memory `MemorySessionStorage` (subagent results aren't persisted to the parent jsonl —
  only the tool result is), runs to completion, returns the final assistant text.

The parent jsonl gets one `tool_result` entry containing the subagent's output (so resume
replay works). Subagent intermediate steps are NOT persisted to the parent jsonl; they emit
events under a `subagent.{id}.` prefix for observability in [[14-observability]].

## Stability

- Subagent runtime is fully isolated: its own `Agent`, `CancellationToken`, tool registry,
  permission policy.
- Parent abort cascades: cancelling the parent cancels all live subagents.
- A subagent that loops or exceeds its `max_iters` returns a partial result with a marker, not
  a hang.
- Concurrent cap defaults to 4 — protects API rate limits + local resource use.

## Extensibility

- New subagent specs are added via:
  - `Registry::register_builtin(spec)` in code,
  - `~/.theway/subagents/<name>.toml` config files,
  - extensions (Tier 4) calling `register_subagent`.
- Subagent permission policy can be *more* restrictive than parent — e.g. an `explore`
  subagent might be `read-only`.

## Performance

- Subagents run on the same tokio runtime; tool calls inside them parallelise normally.
- Their token cost rolls up into the parent's `/cost` accounting (tagged by subagent type).

## Testing

| Layer | What |
|---|---|
| **unit** | Spec lookup; concurrent-cap admission control; result aggregation. |
| **integration** | `Task` tool with a faux model that immediately emits a final text; result reaches the parent's next-turn message history. |
| **e2e** | Parent agent fires three `Task` calls in one turn (parallel); all three faux subagents run concurrently; parent receives all three tool results before generating its next assistant turn. Cancelling the parent mid-flight cancels all subagents (verified via cancellation token receivers). |

## Acceptance criteria

- `Task` tool present in `/tools` listing.
- `subagent_type` referencing a non-existent spec returns a clear tool error.
- Three concurrent subagents complete and their results all reach the parent.
- Parent Ctrl-C cancels every running subagent.
- Subagent cost is included in `/cost` and tagged by subagent type.

## Out of scope

- Cross-machine subagents (RPC to remote agents).
- Persistence of subagent intermediate steps to disk (only summary survives).
- Recursive sub-subagents — guarded by depth cap (default 2).
