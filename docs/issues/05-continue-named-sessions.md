# `--continue`, named sessions, `/share` export

> Parent: master roadmap issue.
> Tier: 2 (session/state).

## Goal

`--resume` requires the user to remember an id or pass `--resume-id`. Mature agents have:

- `theway --continue` (alias `-c`) → resume the most recent session for this cwd.
- `theway --continue <name>` → resume by user-assigned name (slug index in a sidecar).
- `theway name <name>` (or `/name <name>` inside the REPL) → tag the current session with a name.
- `/share` → export the active branch transcript to HTML (and optionally upload to a paste
  service / Gist).
- `theway list --all` → list across cwds (filtered by the global session root).

## Architecture

```
theway-core/src/session/
  named.rs       NamedIndex — flat JSON sidecar mapping name → session_id
  export/        HTML renderer (Markdown → HTML via pulldown-cmark + a tiny CSS bundle)
```

Naming is a separate `~/.theway/agent/sessions/<cwd-hash>/names.json` sidecar — atomic write
(`rename`-temp pattern), append-only conceptually (renames produce a new entry, old name still
resolvable but marked stale).

`--continue` without args picks the session with the highest `mtime` in the cwd directory.

`/share` writes `~/.theway/exports/<session-id>.html` and (when an upload backend is configured)
posts the file to a paste service. Backend is pluggable (Gist by default if `gh` is available;
otherwise local-only).

## Stability

- Atomic named-index writes — rename-temp + fsync the directory.
- HTML export streams; never holds the full transcript in memory (relevant for long sessions).
- Upload backends time-out at 10s, never block the REPL.
- If a name collides on insert, the operation fails loudly; user picks a different one.

## Extensibility

- Export format trait: `ExportFormat::Html`, `ExportFormat::Markdown`, future `Pdf`.
- Upload backend trait: `Gist`, `S3`, `LocalOnly`. Default is `LocalOnly` until configured.

## Performance

- Listing across cwds traverses the global session directory once and reads metadata only
  (first line of each jsonl file). Caches by `(path, mtime)`.
- HTML export uses `pulldown-cmark` with a fixed-size lookahead buffer.

## Testing

| Layer | What |
|---|---|
| **unit** | NamedIndex round-trips with concurrent writers; mtime-sort tiebreak rules; HTML escaper for code blocks. |
| **integration** | Two sessions in the same cwd; `--continue` resolves to the later one. `--continue myname` after `/name myname` resolves to that session. |
| **e2e** | PTY: `theway -c` after a fresh exit resumes the previous session and replays. `/share` produces an HTML file that opens with the right transcript order. `theway list --all` lists sessions across two cwds. |

## Acceptance criteria

- `theway --continue` (no arg) resumes the most recent session for the cwd.
- `theway --continue name` resumes the named session even after another newer session exists.
- HTML export round-trips: every assistant text block, tool call/result, model-change marker
  appears in the output.
- Renaming an existing session keeps the old name resolvable until explicitly purged.

## Out of scope

- Cross-cwd merge / move of a session.
- Cloud-sync of sessions (encrypted, multi-device) — separate roadmap item.
- Upload backend configuration UI — config file only in v1.
