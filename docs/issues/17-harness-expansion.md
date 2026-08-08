# AgentHarness expansion: first-class skills, templates, hooks, branch summarization

> Parent: master roadmap issue.
> Tier: 4 (framework depth) — priority bumped to top-of-stack per maintainer direction
> (2026-05-20).

## Goal

The Phase 1-5 harness refactor landed the foundation (Agent + Session + 4 hooks + auto-
compaction + AgentSession auto-retry + rehydration). This issue tracks the next layer of
`theway-core::AgentHarness` capabilities to bring it to parity with the TS `AgentHarness`
and beyond:

1. **First-class skills** — owned by `AgentHarness`, not a separate extension (links to #10
   Part A).
2. **Prompt-template registry** — named templates with arg substitution; `prompt_from_template`
   already exists, formalize it.
3. **Hook bus expansion** — typed events bus that extensions and observers subscribe to:
   `session_start`, `before_turn`, `after_turn`, `before_tool_call`, `after_tool_call`,
   `agent_end`, `compaction`, `branch`.
4. **Branch summarization** — when a session forks, summarize the parent leaf into a single
   message that the new branch sees as starting context.
5. **Multi-leaf session model** — explicit `active_leaf_id` in `Session`; `move_to(leaf, ...)`,
   `branch_from(leaf, prompt)`, `list_leaves()`.
6. **Determinism guarantees** — every operation that mutates session state is exercised by a
   round-trip test: write → drop → re-open → assert identical state.

## Architecture

```
theway-core/src/harness/
  agent_harness.rs    expand: skills field, templates field, event_bus field
  templates.rs        PromptTemplate { name, body, args }; registry with precedence
  events.rs           EventBus<HarnessEvent>; typed channel per event kind
  branching.rs        branch_from / move_to / summarize_parent_leaf
  session/
    leaves.rs         multi-leaf representation, deterministic ordering
```

Existing `AgentSession` (in `theway`) and the CLI sit unchanged on top — they
consume the new APIs.

## Stability

- Multi-leaf session moves are transactional: write the new active_leaf row, fsync, only then
  update in-memory state.
- Branch summarization invokes the LLM; failures fall back to "no summary, just attach
  parent's last user message as context".
- Event bus subscribers are isolated — one panicking subscriber doesn't block delivery to
  others (use `tokio::spawn` with `catch_unwind`).

## Extensibility

- Templates live in `.theway/templates/<name>.md` (and user-global mirror), same three-layer
  precedence as skills.
- The event bus is the canonical extension surface: anything an extension wants to observe is
  an event, not a custom hook.

## Performance

- Event delivery is O(subscribers); subscribers run on their own tasks.
- Branch summarization is a single LLM call; gated by a config knob (off by default if the
  user only ever has one leaf — most users).
- Skill set + template registry are loaded once per session.

## Testing

| Layer | What |
|---|---|
| **unit** | Template arg substitution edge cases; event bus delivery semantics; leaf comparator stability. |
| **integration** | `prompt_from_template("review_pr", { pr: 42 })` resolves and prompts; branching writes the right jsonl rows; summarization fallback works when LLM errors. |
| **e2e** | Multi-leaf session: prompt twice on leaf A, branch to leaf B, prompt once, resume the file → `list_leaves()` returns both with right active marker, `move_to(B)` reflects in next turn's context. Subscribe to `before_tool_call` from a test extension; cancel turn from the subscriber; tool never runs. |

## Acceptance criteria

- `AgentHarness::skills()` returns the loaded skill set; reload survives `--resume`.
- `AgentHarness::templates()` returns the loaded templates.
- `AgentHarness::subscribe(EventKind, handler)` delivers typed events.
- A session with three leaves round-trips through drop / reopen with the same active leaf and
  branch graph.
- Branch summarization can be opted in via `HarnessOptions::summarize_branches = true` and a
  branched session contains a summary message at index 0.

## Out of scope

- Cross-session branch merging.
- Network-distributed agent state.
- Skill / template hot reload (defer).
