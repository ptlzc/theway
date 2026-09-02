import { defineExtension } from "@theway-ai/plugin-sdk";

export default defineExtension((api) => {
  // Config is injected and validated by the host; getConfig returns merged config.
  const config = api.getConfig();

  // Register a cleanup function executed in reverse on unload.
  api.effect(() => () => {
    // No-op disposer used to exercise the disposer queue.
  });

  // Tool registration (object signature).
  api.registerTool({
    name: "fixture_echo",
    label: "Fixture echo",
    description: "Echo the supplied arguments.",
    inputSchema: { type: "object" },
  }, async (invocation) => ({
    content: [{ type: "text", text: `echo ${JSON.stringify(invocation.arguments)}` }],
    details: { greeting: config?.greeting ?? null },
  }));

  // Action registration (object signature).
  api.registerAction({
    name: "fixture_greet",
    description: "Greet the caller.",
    inputSchema: { type: "object" },
  }, async () => JSON.stringify({ greeting: config?.greeting ?? "hello" }));

  // Public event subscription with the `tools/result` surface name.
  api.on("tools/result", (envelope) => {
    const payload = envelope?.payload ?? {};
    return { actions: [{ kind: "emit_diagnostic", payload: {
      code: "lifecycle_status",
      severity: "info",
      message: "fixture",
      details: { toolName: payload.toolName ?? "" },
    } }] };
  });
});
