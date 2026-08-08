# Structured logging, `/diag`, latency/cost display, error-report bundling

> Parent: master roadmap issue.
> Tier: 6 (observability).

## Goal

When a turn goes wrong (timeout, tool error, parse error, retries), the user has no visibility.
There's no canonical log file, no per-turn telemetry, no diagnostic dump.

Ship:

- A `tracing` subscriber wired by default; events emitted by every layer
  (`theway-llm-provider::stream`, `theway-core::agent_loop`, tool executions, retries, MCP RPC).
- Log file at `~/.theway/logs/<session-id>.log` (rotating, size-capped); structured JSON lines.
- `/diag` slash command — dumps:
  - active model, thinking level, token budget remaining
  - per-tool call counts + average duration for the session
  - last error if any, with stack trace
  - extension / MCP server status
- Status line (bottom of TUI) shows turn latency + cumulative cost (driven by [[06-token-cost-budget]]).
- `/bug-report` — bundles the diag dump + the last K log entries (redacted of API keys) into a
  `.tar.gz` for issue attachments.

## Architecture

```
theway-core/src/observability/
  mod.rs           Subscriber bootstrap (env filter, console layer for verbose, file layer
                    always)
  spans.rs         Standard span names + fields (turn_id, session_id, model, tool)
  diag.rs          Snapshot generator
  bug_report.rs    Redactor + tar+gz writer
```

`theway-llm-provider` and `theway-core` emit spans + events through `tracing`. The coding-agent
configures the subscriber. Span fields are stable and documented.

Redaction list (regex):

- `sk-[A-Za-z0-9]{20,}` (OpenAI / Anthropic / Stripe-style)
- `AKIA[0-9A-Z]{16}` (AWS access keys)
- `gho_[A-Za-z0-9]{36}` (GitHub tokens)
- `Bearer [A-Za-z0-9._\-]+`
- per-provider auth.json fields

## Stability

- Log file rotation is a single-process operation guarded by an `fs2` advisory lock on the log
  dir.
- File writes are non-blocking via `tracing-appender::non_blocking`.
- If the subscriber fails to initialize, fall back to stderr; do NOT block the agent.
- `/bug-report` redaction is mandatory; tests assert no test secret leaks into the bundle.

## Extensibility

- Subscriber layers can be added at runtime (e.g. extensions add OTLP export).
- Diag snapshot returns a serializable struct; future UIs (Web UI) render it natively.

## Performance

- Logging adds <5% overhead in benchmarks; verified by a micro-bench.
- Bug-report bundling streams the file rather than reading whole.

## Testing

| Layer | What |
|---|---|
| **unit** | Redactor catches the documented patterns; rotator opens a new file at the configured size threshold. |
| **integration** | Run a synthetic turn; assert at least one `agent.turn` span with model + duration is emitted; assert `tool.call` event present per tool. |
| **e2e** | Drive a session that errors mid-turn; `/diag` shows the error; `/bug-report` produces a bundle whose contents — searched with ripgrep — do NOT contain any of the test fixture's secret strings, and DO contain the expected log lines. |

## Acceptance criteria

- Default install writes `~/.theway/logs/<session-id>.log` for every session.
- `/diag` returns within 100ms with the documented sections.
- `/bug-report` produces a tar.gz with redacted contents and no plaintext secrets.
- Status line shows live latency for the in-flight turn (updates per second).

## Out of scope

- Remote telemetry (OTLP export) — design here, ship as an extension under [[09-extensions-skills]].
- Long-term log retention policies — user's responsibility.
- Privacy-mode toggle that disables logging entirely — could be added trivially later.
