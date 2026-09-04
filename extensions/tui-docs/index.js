import { defineExtension } from "@theway-ai/plugin-sdk";

// tui-docs: injects the TUI documentation found in the workspace into the
// model context as ordered prompt sections. The document file stays the
// single source of truth — the plugin re-reads it whenever the daemon loads
// (or reloads) this package, so doc edits land without touching the plugin.

// Ordered candidate paths for the TUI documentation, relative to the
// workspace root. The first readable file wins.
const DOC_PATHS = [".agents/overview/tui.md", "docs/tui.md"];

// The host rejects one prompt section's text over 16 KiB (16384 bytes).
// Split below that, on line boundaries, so a growing document shards
// cleanly and each part stays valid.
const MAX_SECTION_BYTES = 16000;

function utf8Bytes(text) {
  let bytes = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0);
    bytes += code <= 0x7f ? 1 : code <= 0x7ff ? 2 : code <= 0xffff ? 3 : 4;
  }
  return bytes;
}

function splitSections(text, maxBytes) {
  const lines = text.split("\n");
  const parts = [];
  let current = "";
  for (const line of lines) {
    if (current === "") {
      current = line;
      continue;
    }
    const candidate = current + "\n" + line;
    if (utf8Bytes(candidate) > maxBytes) {
      parts.push(current);
      current = line;
    } else {
      current = candidate;
    }
  }
  if (current !== "") {
    parts.push(current);
  }
  return parts;
}

export default defineExtension(async (api) => {
  let text = null;
  for (const path of DOC_PATHS) {
    try {
      const candidate = await api.workspace.readText(path);
      if (candidate && candidate.trim() !== "") {
        text = candidate;
        break;
      }
    } catch (_error) {
      // unreadable / missing — try the next candidate
    }
  }
  if (text === null) {
    api.log(
      "warn",
      `tui-docs: no TUI documentation found at ${DOC_PATHS.join(" or ")}; ` +
        "no prompt section registered",
    );
    return;
  }
  const parts = splitSections(text, MAX_SECTION_BYTES);
  parts.forEach((part, index) => {
    api.registerPromptSection({
      sectionId: `tui-docs-overview-${index + 1}`,
      text: part,
      priority: 0,
      scope: "session",
    });
  });
  api.log(
    "info",
    `tui-docs: injected ${parts.length} prompt section(s) (${utf8Bytes(text)} bytes)`,
  );
});
