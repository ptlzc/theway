import { defineExtension } from "@theway-ai/plugin-sdk";

// tui-docs: registers one small prompt-section pointer telling the model
// where the theway TUI documentation lives — it never injects the document
// body. A workspace copy wins (checked through api.workspace.read);
// otherwise the pointer names the copy bundled with the `theway` binary and
// materialized to $THEWAY_DIR/docs/tui.md (default ~/.theway/docs/tui.md)
// by the client on startup.

const WORKSPACE_CANDIDATES = [".agents/overview/tui.md", "docs/tui.md"];
const INSTALLED_DOC = "$THEWAY_DIR/docs/tui.md (default ~/.theway/docs/tui.md)";

export default defineExtension(async (api) => {
  let workspacePath = null;
  for (const candidate of WORKSPACE_CANDIDATES) {
    try {
      const text = await api.workspace.readText(candidate);
      if (text && text.trim() !== "") {
        workspacePath = candidate;
        break;
      }
    } catch (_error) {
      // unreadable / missing — try the next candidate
    }
  }
  const location =
    workspacePath !== null
      ? `\`${workspacePath}\` (workspace copy)`
      : `\`${INSTALLED_DOC}\` (bundled with the binary)`;
  api.registerPromptSection({
    sectionId: "tui-docs-pointer",
    text:
      `Theway TUI documentation is available at ${location}. ` +
      "Use the read tool to consult it when you need details about theway " +
      "TUI behavior, keybindings, terminal layout, or client architecture.",
    priority: 0,
    scope: "session",
  });
  api.log("info", `tui-docs: prompt pointer registered → ${location}`);
});
