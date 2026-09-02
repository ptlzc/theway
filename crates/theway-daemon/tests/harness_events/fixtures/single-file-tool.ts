export const kind = "tool";

export default {
  setup(api) {
    const config = api.getConfig();

    api.effect(() => () => {
      // No-op disposer to exercise the disposer queue.
    });

    api.registerTool({
      name: "single_file_echo",
      label: "Single-file echo",
      description: "Echo the supplied arguments from a single-file kind extension.",
      inputSchema: { type: "object" },
    }, async (invocation) => ({
      content: [{ type: "text", text: `echo ${JSON.stringify(invocation.arguments)}` }],
      details: { greeting: config?.greeting ?? null },
    }));

    api.on("tools/result", (envelope) => {
      const payload = envelope?.payload ?? {};
      return { actions: [{ kind: "emit_diagnostic", payload: {
        code: "lifecycle_status",
        severity: "info",
        message: "fixture",
        details: { toolName: payload.toolName ?? "" },
      } }] };
    });
  },
};
