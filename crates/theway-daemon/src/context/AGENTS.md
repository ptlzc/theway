# daemon/src/context — Context projection module

This module is the daemon-side entry point for turning a session into the
context bundle consumed by the agent harness. It reads the persisted session
log, derives a lineage block, and composes the system prompt. It never mutates
session entries.

## Components

| File | Responsibility |
| --- | --- |
| `mod.rs` | Module declarations and test bridge. |
| `system_prompt.rs` | Base system prompt + working directory + memory + optional lineage block. |
| `lineage.rs` | Collapse lineage/handoff block (identity + tool guidance only). |
| `service.rs` | `ContextService::load(session) -> ContextBundle` single entry point. |

## Context flow

```text
Session entries (append-only log)
  -> session.build_context()            // core: entries -> AgentMessage list
  -> session.compact_context()          // core: read compact_context custom entry
  -> session.collapse_node_id()         // core: read collapseNodeId metadata
  -> render_lineage(...)                // daemon: identity + tool guidance
  -> compose_system_prompt(...)         // daemon: base prompt + cwd + memory + lineage
  -> ContextBundle { system_prompt, messages }
```

`ContextBundle.messages` still contains `AgentMessage` values. The provider
request is materialized later by `theway_core::default_convert_to_llm`, which
maps the known custom summary roles to framed user text.

## Invariants

1. Session entries are the canonical log. Context projection only reads them.
2. The compact summary is injected exactly once, as a
   `collapse_context` custom message in `messages`.
3. The system prompt carries lineage identity and tool guidance only; it does
   not repeat `compactText`.
4. Raw transcripts stay out of the default context and remain available
   through `session_graph_read`.

## Example contexts

### 1. Normal session, no collapse lineage

```text
You are theway, a minimal coding assistant running in a terminal. You have access to the following tools: read, bash, edit. ...

Current working directory: /home/user/project

Remember: keep commit messages conventional.
```

`messages`:

```json
[
  { "role": "user", "content": "add a login page", "timestamp": 123 }
]
```

### 2. Collapse child session

After `/collapse`, the new session has a `compact_context` entry and
`collapseNodeId` metadata. The system prompt gains a lineage block but no
summary text:

```text
You are theway, a minimal coding assistant running in a terminal. You have access to the following tools: read, bash, edit, session_graph_list, session_graph_read, session_graph_status, session_graph_wait, session_graph_attach. ...

Current working directory: /home/user/project

## Session lineage

Collapse node: node-01JEXAMPLE0000000000000000
This session continues from session-123.
Use session_graph_list / session_graph_read / session_graph_status / session_graph_wait / session_graph_attach to inspect or take over the old session graph.
```

`ContextBundle.messages` before provider materialization:

```json
[
  {
    "type": "custom",
    "role": "collapse_context",
    "payload": {
      "summary": "Explored auth module; decided token refresh strategy; next step is the login form."
    }
  }
]
```

After `default_convert_to_llm`:

```text
[Previous session compact summary]
Explored auth module; decided token refresh strategy; next step is the login form.
```

### 3. Collapse into an existing session (`into_session_id`)

The existing messages remain on the active branch, then the collapse context
entries are appended after them. System prompt is the same lineage block as
case 2.

`ContextBundle.messages` before provider materialization:

```json
[
  { "role": "user", "content": "start the refactor", "timestamp": 1 },
  { "role": "assistant", "content": "beginning refactor...", "timestamp": 2 },
  {
    "type": "custom",
    "role": "collapse_context",
    "payload": {
      "summary": "Old session covered the parser rewrite and its tests."
    }
  }
]
```

### 4. Collapse with `--adopt`

Prompt shape is unchanged from case 2. The only difference is runtime state:
active DAG runs and subagent jobs are re-homed to the new session, so the
`dag_*` tools now operate on the migrated runs while `session_graph_*` tools
can still read the collapse node.

### 5. Session with no collapse metadata

`render_lineage` returns `None`, so `compose_system_prompt` emits no
`## Session lineage` block. The context is the normal base prompt + cwd +
memory only.
