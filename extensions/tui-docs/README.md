# tui-docs

Runtime extension package that injects the project's TUI documentation into
the model context as ordered prompt sections.

At package load the plugin reads the first readable candidate from the
workspace root and registers it via `registerPromptSection`:

1. `.agents/overview/tui.md` — theway repo's agent-facing TUI architecture doc
2. `docs/tui.md` — a committed docs location, when present

The document file stays the single source of truth: edits land on the next
daemon extension reload (`/extension-reload`), no plugin change needed. If no
candidate is readable the plugin logs a warning and registers nothing. The
host caps one prompt section's text at 16 KiB, so longer documents are sharded
on line boundaries into `tui-docs-overview-N` sections that the host appends
in order to the request's `systemInstructions`.

## Install

Copy the directory into a project or global extension root, then record trust
for the `workspace.read` permission and reload the catalog:

```bash
cp -r extensions/tui-docs <cwd>/.theway/extensions/tui-docs
# in the TUI: /extension-trust → trust the project with workspace.read,
# then /extension-reload (or restart the daemon)
```

Verified by `crates/theway-daemon/tests/tui_docs_extension.rs` (document
injection + missing-document no-op).
