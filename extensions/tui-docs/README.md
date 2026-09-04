# tui-docs

Runtime extension package that tells the model where the theway TUI
documentation lives, through one small prompt-section pointer appended to the
request's `systemInstructions`. It never injects the document body, so the
per-request cost is a single short sentence; the model reads the file with the
read tool only when it actually needs TUI details.

Pointer resolution at package load:

1. A workspace copy, when readable: `.agents/overview/tui.md`, then
   `docs/tui.md` (checked via `api.workspace.read`).
2. Otherwise the installed copy: `$THEWAY_DIR/docs/tui.md` (default
   `~/.theway/docs/tui.md`), which the `theway` client bundles in its binary
   (theway-tui's `docs/theway-config.md`, the LLM-facing configuration guide)
   and materializes on startup — every install method ships it, no extra
   step needed.

If the file is missing at read time the model simply moves on; the pointer
itself is always registered.

## Install

Copy the directory into a project or global extension root, then record trust
for the `workspace.read` permission and reload the catalog:

```bash
cp -r extensions/tui-docs <cwd>/.theway/extensions/tui-docs
# in the TUI: /extension-trust → trust the project with workspace.read,
# then /extension-reload (or restart the daemon)
```

Verified by `crates/theway-daemon/tests/tui_docs_extension.rs` (workspace
pointer + installed-copy fallback), and the bundled-doc materialization by
`crates/theway-tui/src/tui_docs.rs` unit tests.
