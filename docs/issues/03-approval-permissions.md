# Approval & permission system

> Parent: master roadmap issue.
> Tier: 1 (daily UX, safety).

## Goal

Right now `bash`, `edit`, `write` run unprompted. For a daily-driver agent that is unsafe
(`rm -rf`, accidental overwrites, secrets-leaking commands). Ship a layered approval system:

- **Per-tool permission policy**: `auto` | `prompt` | `deny`. Default for `read` / `ls` /
  `grep` / `find` is `auto`; for `write` / `edit` / `bash` is `prompt`.
- **Diff preview** before `write` / `edit` is applied — user sees the unified diff and confirms.
- **Dangerous bash detection**: regex/AST scan for `rm -rf`, `sudo`, `curl … | sh`, network
  egress, etc. Forces `prompt` even if policy is `auto`.
- **Per-session allow-list**: `[A]lways allow for this session` adds the matched pattern to a
  session-scoped allow set. Stored in session jsonl so it survives `--resume`.
- **Per-tool, per-arg allow rules** in `~/.theway/permissions.toml` (e.g. allow `bash` for `cargo *`
  but prompt for `rm *`).
- **Approval mode flag**: `theway --approval=auto|prompt|never` global override (CI/headless).

## Architecture

```
theway-core/src/permission/
  mod.rs         PermissionPolicy enum, ApprovalDecision enum
  evaluator.rs   Resolver: given (tool_name, args) → Decision
  rules.rs       loader for permissions.toml (workspace + user precedence)
  danger.rs      bash pattern matcher (regex set + a tiny AST for pipe chains)
```

The `Agent::before_tool_call` hook (already exists) calls `PermissionPolicy::evaluate`. Result
is one of:

- `Allow` — run as-is.
- `AllowWithDiff(preview)` — run iff the preview is approved.
- `Prompt(reason)` — surface to UI.
- `Deny(reason)` — short-circuit, synthesize tool error result.

The UI layer (`theway::tui`) renders the prompt and routes the answer back via an
`Arc<dyn ApprovalUi>` trait so headless runners (RPC/print modes when those return) can plug in
non-TTY approvers (auto-deny, callback hook).

## Stability

- A bug here turns into a "data loss". Default-deny on parse failure, never default-allow.
- Pattern matching is bounded: regex set has explicit DFA size cap; AST parser has 10ms timeout.
- Approval prompt is cancellable via Ctrl-C, which is treated as deny.
- Approval state additions to the session-scoped allow list write through to jsonl
  synchronously before the tool runs — otherwise a crash mid-approval loses the policy.

## Extensibility

- The matcher takes a `Vec<Box<dyn DangerCheck>>` so extensions add detectors (e.g. an MCP
  server might add "this URL is on a blocklist").
- `ApprovalUi` is a trait — TUI today, future Web UI tomorrow.
- Permission rules support globbing on tool args; richer matching (regex, AST predicates) is
  out of scope v1.

## Performance

- Evaluator must run ≤ 1ms p99 — regex set is compiled once, kept on the harness.
- Diff preview generation uses `similar` crate; capped at 10k lines, shows "(truncated)" past
  that.

## Testing

| Layer | What |
|---|---|
| **unit** | Pattern matcher classifies a corpus of safe/dangerous commands; rule loader handles missing files, parse errors, project ↔ user precedence. |
| **integration** | Faux tool that requires approval — `Deny` decision results in a synthetic ToolError reaching the LLM; session jsonl records the allow-list addition after `Always allow`. |
| **e2e** | PTY: agent proposes `bash rm -rf /tmp/x`; UI shows danger banner + diff-preview prompt; reject → tool result is "denied by user". Accept → result is the real exit code. Restart with `--resume`; previously-allowed pattern is still allowed (no prompt). |

## Acceptance criteria

- `theway` with default permissions blocks an LLM-proposed `rm -rf` until the user confirms.
- `theway --approval=never` denies every prompt-required tool without UI interaction.
- `~/.theway/permissions.toml` allow-rule for `bash cargo *` skips the prompt for matching args.
- Approving "always" persists into the session jsonl and survives `--resume`.
- `write`/`edit` show a unified diff before applying; reject leaves the file unchanged.

## Out of scope

- Cluster-wide / org-wide permission policy distribution.
- Cryptographic signing of permissions.toml.
- Cross-references: dangerous-pattern detection in MCP tool calls is handled in
  [[08-mcp-client]] but uses the same evaluator pipeline.
