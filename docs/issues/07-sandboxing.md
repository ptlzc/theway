# Sandboxing: scoped file writes, network egress policy, `.thewayignore`

> Parent: master roadmap issue.
> Tier: 3 (safety).

## Goal

Today nothing stops `write`/`edit` from clobbering `/etc/hosts` or `bash` from `curl`-ing a
secret. Ship a layered, opt-in sandbox:

- **Workspace root**: `theway` discovers the workspace root (`.git`, `package.json`, `Cargo.toml`,
  …). Default `write`/`edit` policy: deny any path outside root. Override per session via
  `/allow-write <path>`.
- **`.thewayignore`** (and respect `.gitignore`): paths matching ignore patterns are denied for
  write *and* default-excluded from `read`/`grep`/`find` traversal.
- **Network egress policy** for `bash`: `allow_net = none | allow_list | all`. `none` runs the
  command with no network (Linux: unshare net namespace + bridge; macOS: `pfctl` is too heavy,
  fall back to env-var stripping + warning). `allow_list` matches by domain.
- **Hard read-only mode**: `theway --read-only` disables every write tool and the network-egress
  bash entirely.

## Architecture

```
theway/src/sandbox/
  workspace.rs   discover_workspace_root; path.canonicalize().starts_with(root)
  ignore.rs      .thewayignore + .gitignore matcher (re-use `ignore` crate)
  net/           Linux unshare wrapper; macOS warn-only stub; Windows out of scope
  mode.rs        ReadOnly / Normal / Permissive enum
```

Sandbox checks run inside the existing `before_tool_call` hook, ahead of the permission
prompter from [[03-approval-permissions]]. The order matters: sandbox = hard rule, permission =
soft consent.

`.thewayignore` is checked at every tool entry that touches paths. Default deny on a match — user
can `/allow-write <glob>` to add a session-scoped exception (stored in jsonl).

## Stability

- Path canonicalization MUST happen before policy check — symlink games out.
- Linux net-namespace setup is fork+unshare; failures fall back to a clear error, NOT silent
  permissive mode.
- `.thewayignore` regex set is bounded.

## Extensibility

- The sandbox layer exposes a trait `SandboxBackend` so OS-specific implementations live behind
  the same interface. Linux gets a real one in v1; macOS gets a stub; Windows is a no-op for
  now.
- Network allow-list is loaded from config (`~/.theway/sandbox.toml`).

## Performance

- Path checks are O(1) after canonicalization + cached prefix compare.
- Net-namespace cost is one fork per bash invocation; acceptable for the use case.

## Testing

| Layer | What |
|---|---|
| **unit** | `is_within(root, child)` for symlink escapes; `.thewayignore` glob matches; net allow-list parser. |
| **integration** | Tool `write("/etc/hosts")` returns a sandbox-error result without touching the file; `--read-only` makes the agent's `write` call result in a denied ToolResult. |
| **e2e** | (Linux runner) `bash` proposed by the agent with `allow_net=none` cannot resolve DNS (curl returns "could not resolve"); workspace-root sandbox blocks `cd /; rm -rf foo` from affecting `/foo` when the agent is constrained to `/tmp/wsroot`. |

## Acceptance criteria

- `theway --read-only` cannot write files anywhere (verified by an attempted `write` in e2e).
- Writes outside the workspace root are blocked by default.
- `.thewayignore` glob excludes the path from both writes and traversal.
- Linux `allow_net=none` actually severs network for bash subprocesses.

## Out of scope

- Containerized sandboxes (gVisor, runsc) — separate roadmap.
- Cross-platform parity for net policy — Windows + macOS get best-effort only.
- Capability-based filesystem (Landlock) — interesting but not in v1.
