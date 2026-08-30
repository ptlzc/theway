# @theway-ai/sdk Protocol Reference

This document describes the wire protocol and the main typed APIs exposed by
`@theway-ai/sdk`. It is intended for developers integrating with the theway
daemon (`thewayd`).

## Services

The SDK wraps the theway gRPC services:

| Service | Purpose |
| --- | --- |
| `CommandService` | Prompt submission, abort, model/thinking control, control-plane approval |
| `SessionService` | Session lifecycle, snapshot, history, collapse, graph node reads |
| `SettingsService` | Daemon settings |
| `GraphEngineService` | DAG/goal run control and node output |
| `EventService` | Streaming snapshot/event frames |
| `Health` | gRPC health checks |

## Session identity

- New session resource objects use `id`.
- `session_id` is retained on requests and legacy responses for compatibility.
- New clients should prefer `SessionInfo.id` / `SessionSummary.id`.

## SessionSnapshot

`getSnapshot(sessionId)` returns a nested `SessionSnapshot`:

```text
SessionSnapshot
├── session: SessionInfo        # id, name, cwd, created_at, metadata
├── runtime: SessionRuntime     # ModelRef, ThinkingLevel, supported_thinking_levels
├── feed: SessionFeed           # FeedBlock list + incremental cursors
├── graphs: SessionGraphState   # dags, subagents
└── lineage: SessionLineage     # parent_id, child_ids, collapsed_node_ids
```

### Thinking levels

The protocol defines a canonical `ThinkingLevel` enum:

```text
off | minimal | low | medium | high | xhigh
```

A model may not support every level. `SessionRuntime.supported_thinking_levels`
is derived from the model's `thinkingLevelMap`; `thinking_level` must be one of
the supported values.

## Pagination

### ListSessionMessages

Reads the full message history of a session's active branch page by page.

```ts
const page = await client.listSessionMessages('sess-1', 50);
// page.blocks: FeedBlock[] (oldest → newest within the page)
// page.nextBeforeEntryId: string | undefined — pass as beforeEntryId for the
//   previous page
// page.hasMore: boolean
// page.total: number

const older = await client.listSessionMessages('sess-1', 50, page.nextBeforeEntryId);
```

The server caps `limit` at 500. `GetSnapshot` carries the current feed
projection only; full history always goes through `ListSessionMessages`.

### ListSessionGraphNodeMessages

Reads one graph node's structured messages (`FeedBlock` list).

```ts
const res = await client.listSessionGraphNodeMessages(
  'sess-1',
  'node-1',
  0,
  50,
);
```

## Graph nodes

Graph nodes are not all sessions. `SessionGraphNodeType`:

| Type | Meaning |
| --- | --- |
| `SESSION` | A collapsed session |
| `DAG_RUN` | A DAG run |
| `DAG_NODE` | A single DAG node |
| `SUBAGENT_JOB` | A subagent job |
| `GOAL_RUN` | A goal run |

`getSessionGraphNode(sessionId, nodeId)` returns one node's status/summary.
`streamSessionGraphNode(sessionId, nodeId)` returns a stream of
`SessionGraphNodeStreamFrame`:

```text
node: SessionGraphNode          # initial/periodic state
block: FeedBlock                # new structured output
status: SessionGraphNodeStatus  # running/completed/failed/...
```

## Collapse

`collapseSession(request)` turns the current session into a graph node and
creates a new session with a compact context.

```ts
const res = await client.collapseSession({
  sessionId: 'sess-1',
  name: 'phase-2',
  adoptRunningGraphs: false,
});
// res.childSessionId
// res.nodeId
// res.snapshot
```

- Default: the old session's running DAG/subagent graph keeps running.
- `adoptRunningGraphs: true` migrates ownership to the new session.

## Compatibility

- The previous flat state / snapshot-shaped history client methods were
  removed; use `getSnapshot()` + `listSessionMessages()`.
- `session_id` fields on request messages remain the routing identifier.
