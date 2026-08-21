import { defineExtension, type JsonObject } from '@theway-ai/plugin-sdk';

interface NoteArguments extends JsonObject {
  path: string;
  text: string;
}

export default defineExtension(async (api) => {
  api.registerTool<NoteArguments>(
    {
      name: 'workspace-note',
      label: 'Workspace note',
      description: 'Write one note inside the workspace.',
      inputSchema: {
        type: 'object',
        properties: {
          path: { type: 'string' },
          text: { type: 'string' },
        },
        required: ['path', 'text'],
      },
      permission: 'prompt',
    },
    async ({ arguments: args }) => {
      await api.workspace.writeText(args.path, args.text);
      return {
        content: [{ type: 'text', text: `wrote ${args.path}` }],
        details: { path: args.path },
      };
    },
  );

  api.on('before_model_request', { priority: 20 }, ({ payload }) => ({
    actions: [
      {
        kind: 'replace_model_request',
        payload: { request: payload.request },
      },
    ],
  }));
});
