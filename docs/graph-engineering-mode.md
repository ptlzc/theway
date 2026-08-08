# Graph Engineering Mode: 声明式 DAG 子代理编排

> "Any 2+ subtask orchestration is a DAG. Plan the dependency graph once, the engine
> auto-triggers nodes whose prerequisites all succeeded — no manual `task` fan-out."

`graph engineering mode` is the Rust port of the pi `dag-orchestrator` extension: an
in-process DAG engine that runs subagent tasks as a dependency graph and exposes it to
the agent through the `dag_*` tool set. The engine lives in
`crates/core/src/runtime/graph_engineering/` (pure logic — the coding-agent side drives
it through a `NodeLauncher`), the tools in `crates/coding-agent/src/tools/dag_tools.rs`,
and the subagent execution in `tools/{subagent_specs,node_launcher}.rs`.

## Tool surface

| Tool | Purpose |
| --- | --- |
| `dag_plan` | Define a DAG (JSON `nodes[]` or mermaid `graph TD` text) and auto-start it. |
| `dag_status` | Run summary + dependency tree + mermaid (pastes into mermaid.live). |
| `dag_inspect` | Single-node detail: deps, attempts, error, output tail, live preview. |
| `dag_wait` | Event-driven harvest: block until run(s) reach a terminal state (idle watchdog 30s). |
| `dag_retry` | Re-run failed/cancelled nodes + their blocked downstream closure; also restarts a terminal run. |
| `dag_skip` | Mark a node skipped (counts as success for downstream; aborts it if running). |
| `dag_cancel` | Abort the whole run: running jobs cancelled, pending/ready marked cancelled. |

Node states: `[wait]` pending · `[ready]` ready · `[run]` running · `[done]` succeeded ·
`[fail]` failed · `[skip]` skipped · `[cancel]` cancelled. Run summary line:
`dag-1 [name] — done 2/5 · run 1 · ready 1 · cancel 1 · fail 0 · ↑12,345 ↓6,789 · 45.6 tok/s`.

## Quick start

```text
dag_plan(name="migration", mermaid="graph TD\n  A[\"explorer: 调研代码库\"] --> B[\"planner: 计划\"]\n  B --> C[\"executor-coder: 实现\"]\n  C --> D[\"checker: 验证\"]")
```

The engine launches root nodes immediately and auto-triggers the next eligible batch as
prerequisites complete, within `maxConcurrency` (default 10). Harvest with `dag_wait`,
drill into failures with `dag_inspect`, intervene with `dag_retry` / `dag_skip` /
`dag_cancel`.

Mermaid subset: `graph|flowchart TD|TB|LR`, node definitions `A["agent: task"]` (also
`: ` full-width colon), edges `-->` / `-.->`, multi-target `A --> B, C`, `%%` comments.

## Node execution

Each node is a fresh subagent `AgentHarness` (in-memory session, `MemorySessionStorage`
— nothing touches disk). The subagent type resolves through the built-in spec registry
(`tools/subagent_specs.rs`):

| Spec | Tools |
| --- | --- |
| `explorer` | read/ls/grep/find/web_fetch/web_search/git (read-only + web) |
| `planner` | read/ls/grep/find |
| `executor-coder` | full coding set (incl. write/edit/bash/shell) |
| `checker` | read/ls/grep/find/bash/git |
| `general` | read-only set (identical to the `task` tool) |

Unknown agent names are rejected at `dag_plan` time (like the TS extension's config.yaml
validation). Node-level `model` / `thinking` / `timeout` overrides are honored with v1
limitations documented at the launcher: `model` swaps the parent model's id (provider
stays), `thinking` applies when the model declares a `thinkingLevelMap`, `timeout` wraps
the whole run in `tokio::time::timeout`.

## Semantics (1:1 with the TS dag-orchestrator)

- `failFast=false` (default): a failed node cancels only its downstream closure;
  independent branches keep running. `failFast=true`: any failure aborts everything.
- `skipped` counts as success for downstream; `cancelled` counts as blocked.
- Retry replays the affected subgraph: the failed node **and** every node its failure
  had cancelled are reset to pending and re-scheduled; untouched branches stay as-is.
- Session isolation: each run is stamped with the owning session id; `dag_*` tools
  refuse runs owned by another session (multi-agent projects never cross-trigger).
- Persistence: running runs snapshot to `<project>/.pi/graph-engineering-state-<sessionId>.json`
  (10s debounce, non-terminal runs only; the filename deliberately differs from the TS
  `dag-orchestrator-state-*` so both agents can coexist in one project). On the next
  session start the engine restores them — running nodes demote to `ready` and are
  re-launched automatically.

## Known deviations from the TS extension

- `render_tree` has cycle protection (TS would stack-overflow on a cyclic graph; the
  validator rejects cycles anyway).
- mermaid rendering honors the parsed `graph LR` direction (TS ignored it and used the
  `direction` param only).
- No BgJob registry/telemetry: node output tails and token counts live on the node
  itself (launcher-reported) instead of a job registry.
- No TUI widget (`/dag` slash command); `dag_status` covers monitoring.

## Implementation notes

- Engine (`crates/core/src/runtime/graph_engineering/`): `types.rs` (node/run model),
  `graph.rs` (mermaid parse/render, validation, reconcile — the "auto-trigger" state
  derivation), `engine.rs` (scheduler: plan/tick/terminal handling/failFast/retry/skip/
  cancel/wait, `Notify`-based event-driven waiting with a ≤30s idle watchdog),
  `persist.rs` (JSON snapshot + hydrate; running nodes demote to ready on restore).
- Wait-wakeup edge: `notify_waiters` without a registered waiter drops the wake — a
  completion landing between the terminal check and the `notified()` registration is
  picked up on the next sleep cycle (self-healing, ≤30s).
- Shell tools (`tools/shell.rs`): background shells are a process-lifetime registry
  (survive turns, die with the agent process); `kill_shell` kills the process tree —
  Unix `killpg` via `setsid` at spawn, Windows `taskkill /PID /T /F` with
  `CREATE_NO_WINDOW`.
- Windows note: tokio process pipes are blocking-pool backed — an orphan holding the
  pipe write end can hang the drain. The shell spawn path kills the tree via
  `taskkill /T` while the parent is still alive (the same class of issue as the
  pre-existing `runtime::env::native::exec_timeout` test failure on Windows).
