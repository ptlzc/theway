# tui-docs

Runtime extension package that tells the model where the theway
configuration guide lives, through one small prompt-section pointer appended
to the request's `systemInstructions`. It never injects the document body, so
the per-request cost is a single short sentence; the model reads the file
with the read tool only when it actually needs theway configuration details.

Pointer resolution at package load:

1. A workspace copy of the guide, when readable: `docs/theway-config.md`,
   `theway-config.md`, then `docs/tui.md` (checked via `api.workspace.read`).
2. Otherwise the installed copy: `$THEWAY_DIR/docs/tui.md` (default
   `~/.theway/docs/tui.md`), which the `theway` client bundles in its binary
   (theway-tui's `docs/theway-config.md`, the LLM-facing configuration guide)
   and materializes on startup — every install method ships it, no extra
   step needed.

If the file is missing at read time the model simply moves on; the pointer
itself is always registered.

## Install

No install step is needed: the package lives under
`crates/theway-extensions/packages/tui-docs` and the `theway-extensions`
crate embeds it into the daemon, which provisions it into the managed layer
`$THEWAY_DIR/extensions-managed/tui-docs` at startup. Managed packages are
discovered automatically, granted their declared permissions
(`workspace.read`) without a trust record, and are user read-only — every
install method (source, `scripts/install.sh`, crates.io) ships it with zero
setup.

Manual copies still work for experimentation:

```bash
cp -r crates/theway-extensions/packages/tui-docs <cwd>/.theway/extensions/tui-docs
# in the TUI: /extension-trust → trust the project with workspace.read,
# then /extension-reload (or restart the daemon)
```

A project or user package with the same id shadows the managed copy.

Verified by `crates/theway-daemon/tests/tui_docs_extension.rs` (workspace
pointer + installed-copy fallback), the managed-layer provisioning by
`crates/theway-extensions` unit tests, and the bundled-doc materialization by
`crates/theway-tui/src/tui_docs.rs` unit tests.
