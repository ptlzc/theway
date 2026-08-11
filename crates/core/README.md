# theway-core (Rust)

Rust port of [`@earendil-works/pie-agent-core`](https://github.com/earendil-works/pi) — the
stateful agent runtime layered on top of [`theway-llm-provider`](../llm-provider). Provides the agent loop, tool
execution, session persistence, compaction, skills resolution, and prompt-template plumbing.

## Status

1:1 source-level port with the upstream TS `packages/agent/` as reference. All four target subsystems —
**harness, Agent loop, session, compaction** — are functional and covered by integration tests
against a synthetic stream.

| Layer | Status |
|-------|--------|
| Core types (`types.rs`) | implemented |
| `Agent` + `run_agent_loop` | implemented (sequential tool execution, lifecycle events, abort, queues) |
| `AgentHarness` | implemented (composes Agent + Session + skills + system prompt, persists to session) |
| Skills (`agent/skills.rs`) | implemented + tested against tempdirs |
| System-prompt builder | implemented |
| Prompt templates | implemented (loader stub + `{{var}}` interpolation) |
| Session repo (jsonl + memory) | implemented; both backends share `SessionStorage` trait |
| Compaction + branch summarization | implemented (token estimation, cut-point, generate_summary, compact) |
| Native env adapter | implemented (std::fs + tokio::process) |

```
cargo test     # lib unit tests + run_loop / harness_e2e / session / skills / permission suites
```

## Layout

```
src/
  lib.rs                 barrel + re-exports (AgentHarness, compaction, …)
  types.rs               AgentMessage / AgentState / AgentEvent / AgentTool / AgentLoopConfig
  agent.rs               bare Agent state machine (always on)
  agent/
    assembly.rs          AgentHarness composer (single file, AGENTS.md exception)
    run_loop/            runAgentLoop (llm.rs / tools.rs / utils.rs)
    compaction/          auto-compaction + cut point
                         (algorithm.rs / compaction.rs / triggers.rs / branch_summarization.rs)
    session/             session shape + jsonl/memory repos + uuid + repo_utils
    env/                 native std::fs ExecutionEnv adapter
    hooks/               lifecycle hooks
    utils/               truncate.rs
    cost.rs              token/cost tracking
    messages.rs          custom AgentMessage variant helpers
    permission.rs        dangerous-tool permission policy
    skills.rs            loadSkills (SKILL.md discovery)
    system_prompt.rs     formatSkillsForSystemPrompt
    types.rs             ExecutionEnv, Skill, repo interfaces
  multiagent/            DAG orchestration (graph/ + registry/)
  proxy.rs               re-exports from theway-llm-provider (HTTP_PROXY helpers)
  node.rs                native entry wiring the std::fs env adapter
  skills_state.rs
  tools/                 core tool implementations (dag_tools/, install_skill/)
```
